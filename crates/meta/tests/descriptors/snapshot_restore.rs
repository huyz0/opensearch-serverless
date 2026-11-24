//! docs/specs/snapshot-restore AC-1, AC-3, AC-4, AC-5: one index restored
//! by copying its captured files, per backend, with exact request counts.

use oss_blob::S3Op::{Delete, Get, GetRange, List, Put};
use oss_blob::{BlobStore, ByteRange, SliceSource};
use oss_format::{CommitManifest, FileLocation, RestoreIndex, ShardRef};
use oss_meta::{CopyBuffers, DeleteMode, RestoreError, SnapshotRestorer};

use crate::{BASE, Backend, all, counted};

/// A record whose index `logs` (uuid `U1`) has shard 0: `_0.si` whole in
/// term 3 (10 bytes) and `_0.cfs` packed in `t=3/pack` at 4 for 6 bytes.
const RECORD: &[u8] = br#"{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,
  "indices":{"logs":{"uuid":"U1","number_of_shards":1,"mapping":"{\"p\":1}","settings":{"index":{"x":1}},
    "shards":[{"shard":0,"term":3,"files":{"_0.si":"t=3","_0.cfs":"t=3/pack#4:6"},"lengths":{"_0.si":10,"_0.cfs":6}}]}}}"#;

fn put(store: &dyn BlobStore, key: &str, bytes: &[u8]) {
    store
        .put(key, &mut SliceSource::new(bytes), &mut [0u8; 64])
        .unwrap();
}

fn read(store: &dyn BlobStore, key: &str) -> Vec<u8> {
    let mut out = Vec::new();
    store
        .get(key, ByteRange::Full, &mut [0u8; 64], &mut |c: &[u8]| {
            out.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
    out
}

fn register(store: &dyn BlobStore, key: &str) -> Vec<u8> {
    let mut buf = vec![0u8; 1 << 16];
    let got = store.read_register(key, &mut buf).unwrap().unwrap();
    buf.truncate(got.len);
    buf
}

fn seed(b: &Backend) {
    let from = ShardRef {
        index: "logs",
        uuid: "U1",
        shard: 0,
    };
    let base = from.base_path(BASE).unwrap();
    put(&*b.store, &format!("{base}t=3/_0.si"), b"0123456789");
    put(&*b.store, &format!("{base}t=3/pack"), b"abcdefghijklmnop");
}

fn restore(
    b: &Backend,
    target: &str,
    uuid: &str,
    chunk: usize,
    record: &[u8],
) -> Result<(), RestoreError> {
    let d = b.descriptors(DeleteMode::Conditional);
    let source = RestoreIndex::find(record, "logs").unwrap().unwrap();
    let (mut read, mut write) = (vec![0u8; chunk], vec![0u8; chunk]);
    SnapshotRestorer::new(std::sync::Arc::clone(&b.store), BASE).restore_index(
        &d,
        &source,
        target,
        uuid,
        1_000,
        &mut vec![0u8; 1 << 16],
        &mut CopyBuffers {
            read: &mut read,
            write: &mut write,
        },
    )
}

#[test]
fn an_index_is_restored_by_copying_each_file_and_landing_the_manifest() {
    for b in all() {
        seed(&b);
        let (done, n) = counted(b.meter(), || restore(&b, "logs2", "U2", 4, RECORD));
        done.unwrap();
        // Create the descriptor: one PUT; each file: one PUT and a read for
        // each 4 bytes (3 for 10 bytes, 2 for 6); the manifest: one create.
        let gets: u64 = n
            .iter()
            .filter(|(op, _)| matches!(op, Get | GetRange))
            .map(|(_, c)| c)
            .sum();
        assert_eq!(gets, 5, "{:?} {n:?}", b.kind);
        assert!(n.iter().all(|(op, _)| *op != List), "{:?}", b.kind);
        assert!(!n.iter().any(|(op, _)| *op == Delete), "{:?}", b.kind);
        let puts: u64 = n.iter().filter(|(op, _)| *op == Put).map(|(_, c)| c).sum();
        assert!(puts >= 4, "{:?} {n:?}", b.kind);

        let to = ShardRef {
            index: "logs2",
            uuid: "U2",
            shard: 0,
        };
        let base = to.base_path(BASE).unwrap();
        assert_eq!(read(&*b.store, &format!("{base}t=1/_0.si")), b"0123456789");
        assert_eq!(read(&*b.store, &format!("{base}t=1/_0.cfs")), b"efghij");
        let manifest = register(&*b.store, &to.manifest_key(BASE).unwrap());
        let parsed = CommitManifest::parse(&manifest).unwrap();
        assert_eq!(parsed.term, 1);
        assert_eq!(parsed.writer, None);
        assert!(
            parsed.wal_ordinal < 0 && parsed.stage_sequence < 0,
            "no log position"
        );
        let whole = FileLocation::Whole { term_dir: "t=1" };
        assert_eq!(parsed.files().len(), 2);
        for file in parsed.files() {
            assert_eq!(file.location, whole, "{}", file.name);
        }
        assert_eq!(parsed.file("_0.si").unwrap().length, Some(10));
        assert_eq!(parsed.file("_0.cfs").unwrap().length, Some(6));
        // The descriptor names the new uuid and the captured mapping.
        let d = b.descriptors(DeleteMode::Conditional);
        let mut buf = vec![0u8; 1 << 16];
        let got = d.get("logs2", &mut buf).unwrap().unwrap();
        assert_eq!(got.uuid, "U2");
        assert_eq!(got.mapping.as_deref(), Some(r#"{"p":1}"#));
    }
}

#[test]
fn a_manifest_a_writer_landed_first_is_a_conflict_and_is_left_alone() {
    for b in all() {
        seed(&b);
        let to = ShardRef {
            index: "logs2",
            uuid: "U2",
            shard: 0,
        };
        b.store
            .create_register(&to.manifest_key(BASE).unwrap(), br#"{"term":9,"files":{}}"#)
            .unwrap();
        let err = restore(&b, "logs2", "U2", 64, RECORD).unwrap_err();
        assert!(
            matches!(err, RestoreError::Conflict { shard: 0 }),
            "{:?} {err}",
            b.kind
        );
        let kept = register(&*b.store, &to.manifest_key(BASE).unwrap());
        assert!(kept.starts_with(br#"{"term":9"#), "{:?}", b.kind);
    }
}

#[test]
fn what_cannot_be_restored_is_refused_before_anything_is_written() {
    let no_length = br#"{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,
  "indices":{"logs":{"uuid":"U1","number_of_shards":1,"shards":[{"shard":0,"term":3,"files":{"_0.si":"t=3"}}]}}}"#;
    let layout = br#"{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,
  "indices":{"logs":{"uuid":"U1","number_of_shards":1,"settings":{"serverless_unit_layout":"{}"},"shards":[]}}}"#;
    for record in [&no_length[..], &layout[..]] {
        for b in all() {
            seed(&b);
            let (err, n) = counted(b.meter(), || restore(&b, "logs2", "U2", 64, record));
            assert!(
                matches!(err, Err(RestoreError::Unsupported(_))),
                "{:?}",
                b.kind
            );
            assert!(n.is_empty(), "{:?} {n:?}", b.kind);
        }
    }
}

#[test]
fn a_source_shorter_than_recorded_fails_and_no_manifest_lands() {
    for b in all() {
        let from = ShardRef {
            index: "logs",
            uuid: "U1",
            shard: 0,
        };
        let base = from.base_path(BASE).unwrap();
        put(&*b.store, &format!("{base}t=3/_0.si"), b"0123");
        put(&*b.store, &format!("{base}t=3/pack"), b"abcdefghijklmnop");
        assert!(
            restore(&b, "logs2", "U2", 4, RECORD).is_err(),
            "{:?}",
            b.kind
        );
        let to = ShardRef {
            index: "logs2",
            uuid: "U2",
            shard: 0,
        };
        let mut buf = [0u8; 256];
        assert!(
            b.store
                .read_register(&to.manifest_key(BASE).unwrap(), &mut buf)
                .unwrap()
                .is_none(),
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn each_failure_says_what_went_wrong() {
    assert_eq!(RestoreError::Unsupported("why").to_string(), "why");
    assert!(
        RestoreError::Conflict { shard: 3 }
            .to_string()
            .contains("shard 3")
    );
}
