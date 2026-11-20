//! docs/specs/point-in-time-capture/SPEC.md: freezing an index, one suite over
//! every backend with request counts, and against an index replaced mid-capture.

use std::sync::{Arc, OnceLock};

use oss_blob::BlobStore;
use oss_blob::S3Op::{Get, GetRange, Head, List, Put};
use oss_format::{
    FileLocation, IndexRecord, ManifestEntry, ShardRef, encode_manifest, looks_like_an_id,
};
use oss_meta::{AliasRecord, CaptureError, DeleteMode, DescriptorStore, PitFile};

use crate::point_in_time::{buf, store_of};
use crate::probe::Quirky;
use crate::{BASE, Backend, all, counted};

const KEEP_ALIVE: u64 = 60_000;
const N_BEFORE_CHECK: u32 = 5;

fn publish(store: &dyn BlobStore, index: &str, uuid: &str, shard: u32, term: u64) {
    let at = ShardRef { index, uuid, shard };
    let entries = [
        ManifestEntry {
            name: "_0.si",
            location: FileLocation::Whole { term_dir: "t=1" }.encode(),
            length: 7,
        },
        ManifestEntry {
            name: "_0.cfs",
            location: FileLocation::Packed {
                term_dir: "t=1",
                blob: "s0",
                offset: 10,
                length: 5,
            }
            .encode(),
            length: 5,
        },
    ];
    let value = encode_manifest(term, Some("n1"), &entries, -1, -1).unwrap();
    store
        .create_register(&at.manifest_key(BASE).unwrap(), &value)
        .unwrap();
}

fn index(d: &DescriptorStore, name: &str, uuid: &str, shards: u32) {
    d.create(&IndexRecord::new(name, uuid, shards), 1, &mut buf())
        .unwrap();
}

fn files() -> Vec<PitFile> {
    vec![
        PitFile {
            name: "_0.cfs".to_owned(),
            location: "t=1/s0#10:5".to_owned(),
            length: Some(5),
        },
        PitFile {
            name: "_0.si".to_owned(),
            location: "t=1".to_owned(),
            length: Some(7),
        },
    ]
}

fn setup(b: &Backend) -> (DescriptorStore, oss_meta::PointInTimeStore) {
    let d = b.descriptors(DeleteMode::Conditional);
    let (_, p) = store_of(Arc::clone(&b.store));
    (d, p)
}

#[test]
fn every_shards_commit_is_frozen_under_a_fresh_id_and_the_view_reads_back() {
    for b in all() {
        let (d, p) = setup(&b);
        index(&d, "logs", "u1", 2);
        publish(&*b.store, "logs", "u1", 0, 3);
        publish(&*b.store, "logs", "u1", 1, 4);
        let (view, sent) = counted(b.meter(), || {
            p.capture(&d, "logs", KEEP_ALIVE, &mut buf()).unwrap()
        });
        assert!(looks_like_an_id(&view.id));
        assert_eq!(view.index, "logs");
        assert_eq!(view.index_uuid.as_deref(), Some("u1"));
        assert_eq!(
            view.expires_at_millis,
            1_000 + KEEP_ALIVE,
            "the store's clock"
        );
        assert!(!view.capturing);
        assert_eq!(
            view.shards
                .iter()
                .map(|s| (s.shard, s.term))
                .collect::<Vec<_>>(),
            [(0, 3), (1, 4)]
        );
        assert!(view.shards.iter().all(|s| s.files == files()));
        assert_eq!(p.read(&view.id, &mut buf()).unwrap(), Some(view.clone()));
        assert!(sent.iter().all(|(op, _)| *op != List), "{sent:?}");
    }
}

#[test]
fn a_shard_that_has_published_nothing_refuses_the_capture_and_leaves_nothing_pinned() {
    for b in all() {
        let (d, p) = setup(&b);
        index(&d, "logs", "u1", 2);
        publish(&*b.store, "logs", "u1", 0, 3);
        match p.capture(&d, "logs", KEEP_ALIVE, &mut buf()) {
            Err(CaptureError::NothingPublished(1)) => {}
            other => panic!("{other:?}"),
        }
        assert!(
            p.live(&mut buf()).unwrap().is_empty(),
            "the marker was released"
        );
    }
}

#[test]
fn a_name_that_is_no_concrete_index_cannot_be_frozen() {
    for b in all() {
        let (d, p) = setup(&b);
        assert!(matches!(
            p.capture(&d, "absent", KEEP_ALIVE, &mut buf()),
            Err(CaptureError::NoSuchIndex)
        ));
        index(&d, "logs", "u1", 1);
        d.create_alias(&AliasRecord::new("a", &["logs"]), 1, &mut buf())
            .unwrap();
        assert!(matches!(
            p.capture(&d, "a", KEEP_ALIVE, &mut buf()),
            Err(CaptureError::IsAlias)
        ));
    }
}

#[test]
fn a_bucketed_index_is_refused_before_anything_is_written() {
    for b in all() {
        let (d, p) = setup(&b);
        let record = IndexRecord {
            bucket: Some("b1".into()),
            ..IndexRecord::new("logs", "u1", 1)
        };
        d.create(&record, 1, &mut buf()).unwrap();
        let (got, sent) = counted(b.meter(), || p.capture(&d, "logs", KEEP_ALIVE, &mut buf()));
        assert!(matches!(got, Err(CaptureError::Unsupported(_))));
        assert_eq!(sent, [(Get, 1)], "one descriptor read: {:?}", b.kind);
    }
}

#[test]
fn an_index_replaced_while_the_capture_began_is_refused_and_released() {
    let slot: Arc<OnceLock<Arc<dyn BlobStore>>> = Arc::new(OnceLock::new());
    let seen = Arc::clone(&slot);
    let quirky = Arc::new(Quirky::hooked(move |_: &dyn BlobStore| {
        let d = DescriptorStore::new(
            Arc::clone(seen.get().unwrap()),
            BASE,
            DeleteMode::Conditional,
        )
        .unwrap();
        d.delete("logs").unwrap();
        index(&d, "logs", "u2", 1);
    }));
    let inner = Arc::clone(&quirky.inner);
    slot.set(Arc::clone(&inner)).ok().unwrap();
    let store: Arc<dyn BlobStore> = quirky.clone();
    let d = DescriptorStore::new(Arc::clone(&store), BASE, DeleteMode::Conditional).unwrap();
    let (_, p) = store_of(store);
    index(&d, "logs", "u1", 1);
    publish(&*inner, "logs", "u1", 0, 3);
    // The descriptor read, the marker's register create and write, then the
    // incarnation check's read: the index is replaced just before it.
    quirky.arm(N_BEFORE_CHECK);
    assert!(matches!(
        p.capture(&d, "logs", KEEP_ALIVE, &mut buf()),
        Err(CaptureError::Replaced)
    ));
    assert!(
        p.live(&mut buf()).unwrap().is_empty(),
        "the marker was released"
    );
}

#[test]
fn the_same_name_made_again_after_the_capture_is_not_the_captured_view() {
    for b in all() {
        let (d, p) = setup(&b);
        index(&d, "logs", "u1", 1);
        publish(&*b.store, "logs", "u1", 0, 3);
        let view = p.capture(&d, "logs", KEEP_ALIVE, &mut buf()).unwrap();
        d.delete("logs").unwrap();
        index(&d, "logs", "u2", 1);
        let read = p.read(&view.id, &mut buf()).unwrap().unwrap();
        assert!(read.pins("logs", Some("u1")));
        assert!(
            !read.pins("logs", Some("u2")),
            "the new index's files are not held"
        );
    }
}

#[test]
fn a_capture_costs_a_constant_and_one_read_a_shard() {
    for b in all() {
        for shards in [1u32, 3] {
            let (d, p) = setup(&b);
            let name = format!("logs{shards}");
            index(&d, &name, "u1", shards);
            for shard in 0..shards {
                publish(&*b.store, &name, "u1", shard, 3);
            }
            let (_, sent) = counted(b.meter(), || {
                p.capture(&d, &name, KEEP_ALIVE, &mut buf()).unwrap()
            });
            // The descriptor twice, a manifest a shard and the view's finish.
            let gets = u64::from(shards) + 3;
            let want = b.budget(
                &[(Get, gets), (Head, 1), (Put, 3)],
                &[(Get, gets), (GetRange, 1), (Head, 1), (Put, 3)],
            );
            assert_eq!(sent, want, "{:?}, {shards} shards", b.kind);
        }
    }
}

#[test]
fn a_refusal_before_the_marker_writes_nothing() {
    for b in all() {
        let (d, p) = setup(&b);
        let (got, sent) = counted(b.meter(), || {
            p.capture(&d, "absent", KEEP_ALIVE, &mut buf())
        });
        assert!(matches!(got, Err(CaptureError::NoSuchIndex)));
        assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
    }
}

/// Two live units, 1 and 3, in four slots; 0 and 2 are the halves of a split of
/// 3 still pending, so a capture that counted them would ask for their commits.
const LAYOUT: &[u8] = br#"{"mode":"adaptive","declared_shards":2,"next_unit_id":4,"units":[{"id":3,"first":"8000000000000000","last":"ffffffffffffffff"},{"id":1,"first":"0","last":"7fffffffffffffff"}],"pending_split":[{"id":3,"first":"8000000000000000","last":"ffffffffffffffff"},{"id":2,"first":"8000000000000000","last":"bfffffffffffffff"},{"id":0,"first":"c000000000000000","last":"ffffffffffffffff"}]}"#;

fn units(d: &DescriptorStore) {
    let record = IndexRecord {
        unit_layout: Some(LAYOUT),
        ..IndexRecord::new("logs", "u1", 4)
    };
    d.create(&record, 1, &mut buf()).unwrap();
}

#[test]
fn an_index_in_units_freezes_its_live_units_and_no_retired_slot() {
    for b in all() {
        let (d, p) = setup(&b);
        units(&d);
        publish(&*b.store, "logs", "u1", 1, 3);
        publish(&*b.store, "logs", "u1", 3, 4);
        let view = p.capture(&d, "logs", KEEP_ALIVE, &mut buf()).unwrap();
        assert_eq!(
            view.shards
                .iter()
                .map(|s| (s.shard, s.term))
                .collect::<Vec<_>>(),
            [(1, 3), (3, 4)],
            "{:?}",
            b.kind
        );
        assert_eq!(p.read(&view.id, &mut buf()).unwrap(), Some(view));
    }
}

#[test]
fn a_live_unit_that_has_published_nothing_refuses_an_index_in_units() {
    for b in all() {
        let (d, p) = setup(&b);
        units(&d);
        publish(&*b.store, "logs", "u1", 1, 3);
        match p.capture(&d, "logs", KEEP_ALIVE, &mut buf()) {
            Err(CaptureError::NothingPublished(3)) => {}
            other => panic!("{other:?}"),
        }
        assert!(p.live(&mut buf()).unwrap().is_empty());
    }
}

#[test]
fn a_layout_that_cannot_be_read_is_refused_before_anything_is_written() {
    for b in all() {
        let (d, p) = setup(&b);
        let record = IndexRecord {
            unit_layout: Some(b"{}"),
            ..IndexRecord::new("logs", "u1", 2)
        };
        d.create(&record, 1, &mut buf()).unwrap();
        let (got, sent) = counted(b.meter(), || p.capture(&d, "logs", KEEP_ALIVE, &mut buf()));
        assert!(matches!(got, Err(CaptureError::Meta(_))), "{got:?}");
        assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
    }
}
