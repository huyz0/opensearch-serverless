//! docs/specs/stock-snapshot-export AC-6, AC-9 and AC-10: a snapshot is
//! published into a stock repository by creating its blobs and then the
//! next `index-N` by a conditional create; a lost race adopts the winner, a
//! second snapshot keeps the first restorable, and the order leaves the
//! repository readable whatever stops a take.

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, ExportStore, S3Op, SliceSource};
use oss_meta::{
    ExportBase, ExportError, ExportIndex, ExportShard, ExportSnapshot, ExportTarget, ExportedFile,
    MAX_ATTEMPTS, StockRepo, open_export_base, publish_snapshot,
};

const PREFIX: &str = "repo/";

fn file(blob: &str, physical: &str) -> ExportedFile {
    ExportedFile {
        name: blob.to_owned(),
        physical_name: physical.to_owned(),
        length: 40,
        checksum: "abc".to_owned(),
        written_by: "10.5.1".to_owned(),
    }
}

fn shards(tag: &str, n: usize) -> Vec<ExportShard> {
    (0..n)
        .map(|s| ExportShard {
            files: vec![
                file(&format!("__{tag}{s}a"), "_0.cfs"),
                file(&format!("__{tag}{s}b"), "segments_3"),
            ],
            index_version: 3,
        })
        .collect()
}

struct Take<'a> {
    name: &'a str,
    uuid: &'a str,
    index: &'a str,
    index_uuid: &'a str,
    shards: Vec<ExportShard>,
}

fn publish(
    store: &Arc<Emulator>,
    base: Option<ExportBase>,
    take: &Take<'_>,
) -> Result<oss_meta::Published, ExportError> {
    let export = ExportStore::new(store.clone(), PREFIX);
    let target = ExportTarget {
        store: &export,
        prefix: PREFIX,
    };
    let base = match base {
        Some(b) => b,
        None => open_export_base(&target)?,
    };
    let id = base
        .index_id(take.index)
        .map_or_else(|| format!("ID{}", take.index), str::to_owned);
    let settings = vec![("index.refresh_interval".to_owned(), "5s".to_owned())];
    let index = ExportIndex {
        name: take.index,
        id: &id,
        uuid: take.index_uuid,
        creation_date_ms: 1_700_000_000_000,
        mapping: Some(r#"{"properties":{"t":{"type":"text"}}}"#),
        settings: &settings,
        shards: &take.shards,
    };
    let indices = [index];
    let snap = ExportSnapshot {
        name: take.name,
        uuid: take.uuid,
        start_ms: 1_700_000_000_100,
        end_ms: 1_700_000_000_900,
        indices: &indices,
    };
    publish_snapshot(&target, base, &snap, &mut [0u8; 64])
}

fn first() -> Take<'static> {
    Take {
        name: "s1",
        uuid: "U1",
        index: "logs",
        index_uuid: "XU1",
        shards: shards("p", 2),
    }
}

fn store() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 20, 1))
}

#[test]
fn a_published_snapshot_reads_back_through_the_restore_readers() {
    let store = store();
    let done = publish(&store, None, &first()).unwrap();
    assert_eq!((done.generation, done.attempts), (0, 1));
    let repo = StockRepo::new(&*store, PREFIX);
    assert_eq!(repo.generation().unwrap(), 0);
    let data = repo.repository_data(0).unwrap();
    let snap = data.snapshot("s1").unwrap();
    let index = data.indices_of(snap).next().unwrap();
    assert_eq!((index.name.as_str(), index.id.as_str()), ("logs", "IDlogs"));
    assert_eq!(index.shard_generations.len(), 2);
    let info = repo.snapshot_info("U1").unwrap();
    assert_eq!(info.name, "s1");
    let meta_id = data.metadata_blob_id(snap, index).unwrap();
    let meta = repo.index_metadata("IDlogs", meta_id).unwrap();
    assert!(meta.mapping.is_some());
    let shard = repo.shard_snapshot("IDlogs", 1, "U1").unwrap();
    let names: Vec<_> = shard.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["__p1a", "__p1b"]);
}

#[test]
fn a_second_snapshot_reuses_the_index_id_and_keeps_the_first_snapshots_files() {
    let store = store();
    publish(&store, None, &first()).unwrap();
    let second = Take {
        name: "s2",
        uuid: "U2",
        shards: shards("q", 2),
        ..first()
    };
    let done = publish(&store, None, &second).unwrap();
    assert_eq!(done.generation, 1);
    let repo = StockRepo::new(&*store, PREFIX);
    let data = repo.repository_data(1).unwrap();
    assert_eq!(data.indices.len(), 1);
    assert_eq!(data.indices[0].id, "IDlogs");
    assert_eq!(data.indices[0].snapshots, ["U1", "U2"]);
    // Both snapshots' shard blobs read, each listing its own files.
    for (uuid, want) in [("U1", "__p0a"), ("U2", "__q0a")] {
        let shard = repo.shard_snapshot("IDlogs", 0, uuid).unwrap();
        assert_eq!(shard.files[0].name, want);
    }
}

#[test]
fn a_take_that_lost_the_create_adopts_the_winner_and_both_snapshots_end_in_the_repository() {
    let store = store();
    let stale = {
        let export = ExportStore::new(store.clone(), PREFIX);
        open_export_base(&ExportTarget {
            store: &export,
            prefix: PREFIX,
        })
        .unwrap()
    };
    publish(&store, Some(stale.clone()), &first()).unwrap();
    let other = Take {
        name: "s2",
        uuid: "U2",
        index: "metrics",
        index_uuid: "XU2",
        shards: shards("m", 1),
    };
    let done = publish(&store, Some(stale), &other).unwrap();
    assert_eq!((done.generation, done.attempts), (1, 2));
    let repo = StockRepo::new(&*store, PREFIX);
    let data = repo.repository_data(repo.generation().unwrap()).unwrap();
    let mut names: Vec<_> = data.snapshots.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["s1", "s2"]);
}

#[test]
fn another_take_giving_the_index_a_different_id_is_a_conflict() {
    let store = store();
    let stale = {
        let export = ExportStore::new(store.clone(), PREFIX);
        open_export_base(&ExportTarget {
            store: &export,
            prefix: PREFIX,
        })
        .unwrap()
    };
    // The winner names its index "logs" too, under the id the stale base
    // would also pick; a different id comes from a differently-based take.
    publish(&store, None, &first()).unwrap();
    let export = ExportStore::new(store.clone(), PREFIX);
    let target = ExportTarget {
        store: &export,
        prefix: PREFIX,
    };
    let settings: Vec<(String, String)> = Vec::new();
    let sh = shards("z", 1);
    let index = ExportIndex {
        name: "logs",
        id: "OTHERID",
        uuid: "XU9",
        creation_date_ms: 1,
        mapping: None,
        settings: &settings,
        shards: &sh,
    };
    let indices = [index];
    let snap = ExportSnapshot {
        name: "s9",
        uuid: "U9",
        start_ms: 1,
        end_ms: 2,
        indices: &indices,
    };
    let got = publish_snapshot(&target, stale, &snap, &mut [0u8; 64]);
    assert!(matches!(got, Err(ExportError::Conflict)), "{got:?}");
}

#[test]
fn a_name_or_uuid_already_held_is_refused_and_leaves_the_repository_as_it_was() {
    let store = store();
    publish(&store, None, &first()).unwrap();
    let objects = store.object_count();
    let same_name = Take {
        uuid: "U2",
        ..first()
    };
    assert!(matches!(
        publish(&store, None, &same_name),
        Err(ExportError::NameTaken)
    ));
    let same_uuid = Take {
        name: "s2",
        ..first()
    };
    assert!(matches!(
        publish(&store, None, &same_uuid),
        Err(ExportError::NameTaken)
    ));
    let repo = StockRepo::new(&*store, PREFIX);
    assert_eq!(repo.generation().unwrap(), 0);
    assert_eq!(
        store.object_count(),
        objects,
        "a refused take writes nothing"
    );
}

fn put_raw(store: &Emulator, key: &str, bytes: &[u8]) {
    store
        .put(key, &mut SliceSource::new(bytes), &mut [0u8; 64])
        .unwrap();
}

#[test]
fn a_take_after_one_that_died_before_the_latest_put_extends_what_it_wrote() {
    let store = store();
    publish(&store, None, &first()).unwrap();
    let second = Take {
        name: "s2",
        uuid: "U2",
        shards: shards("q", 2),
        ..first()
    };
    publish(&store, None, &second).unwrap();
    // The second take died before its put: index.latest still names 0.
    put_raw(&store, "repo/index.latest", &0i64.to_be_bytes());
    let third = Take {
        name: "s3",
        uuid: "U3",
        shards: shards("r", 2),
        ..first()
    };
    let done = publish(&store, None, &third).unwrap();
    assert_eq!((done.generation, done.attempts), (2, 2));
    let repo = StockRepo::new(&*store, PREFIX);
    let data = repo.repository_data(2).unwrap();
    let mut names: Vec<_> = data.snapshots.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["s1", "s2", "s3"]);
}

#[test]
fn the_number_of_creates_a_take_tries_is_bounded() {
    let make = |lost: u32| {
        let store = store();
        for n in 0..lost {
            put_raw(&store, &format!("repo/index-{n}"), b"{}");
        }
        store
    };
    let ok = make(MAX_ATTEMPTS - 1);
    let done = publish(&ok, None, &first()).unwrap();
    assert_eq!(done.attempts, MAX_ATTEMPTS);
    let lost = make(MAX_ATTEMPTS);
    assert!(matches!(
        publish(&lost, None, &first()),
        Err(ExportError::Conflict)
    ));
}

#[test]
fn a_snapshot_costs_its_blobs_and_no_list_register_or_delete() {
    let store = store();
    let before = store.meter().count(S3Op::Put);
    publish(&store, None, &first()).unwrap();
    // Two shards: two snapshot blobs, two generations; one index metadata,
    // the snapshot's own and the global metadata; index-0 and index.latest.
    assert_eq!(
        store.meter().count(S3Op::Put) - before,
        2 + 2 + 1 + 1 + 1 + 1 + 1
    );
    for op in [S3Op::List, S3Op::Delete, S3Op::Copy] {
        assert_eq!(store.meter().count(op), 0, "{op:?}");
    }
}

#[test]
fn the_base_says_where_the_repository_stands_and_which_ids_it_holds() {
    let store = store();
    let open = |s: &Arc<Emulator>| {
        let export = ExportStore::new(s.clone(), PREFIX);
        open_export_base(&ExportTarget {
            store: &export,
            prefix: PREFIX,
        })
        .unwrap()
    };
    let empty = open(&store);
    assert_eq!(empty.generation(), None);
    assert_eq!(empty.index_id("logs"), None);
    publish(&store, None, &first()).unwrap();
    let held = open(&store);
    assert_eq!(held.generation(), Some(0));
    assert_eq!(held.index_id("logs"), Some("IDlogs"));
    assert_eq!(held.index_id("other"), None);
}
