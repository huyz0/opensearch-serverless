//! docs/specs/snapshot-restore AC-2 and AC-7: system indices are refused, and
//! the records and files a Java node reads and writes are the same as this
//! node's.

// The fixtures the Java reads are a few hundred bytes.
#![allow(clippy::disallowed_methods)]

use oss_blob::{BlobStore, SliceSource};
use oss_format::{FileLocation, ShardRef};

use crate::snapshot_restore::{
    RESTORE, Rig, read, read_register, restored_uuid, rig, rig_with, send, taken,
};

/// What a Rust restore wrote for `copy`, for a Java node to read
/// (`fixtures/snapshots/rust-restore/`, `CheckRestore.java`). With
/// `OSS_WRITE_FIXTURES` set the files are rewritten; otherwise the test
/// checks they still parse.
#[test]
fn a_restore_writes_the_descriptor_and_manifests_a_java_node_reads() {
    let r = taken(true);
    let rename = r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#;
    assert_eq!(send(&r, "POST", RESTORE, Some(rename)).status, 200);
    let uuid = restored_uuid(&r, "copy");
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/snapshots/rust-restore");
    let descriptor = read_register(&r, &oss_format::descriptor_key("", "copy").unwrap());
    let manifest = read_register(
        &r,
        &ShardRef {
            index: "copy",
            uuid: &uuid,
            shard: 1,
        }
        .manifest_key("")
        .unwrap(),
    );
    if std::env::var_os("OSS_WRITE_FIXTURES").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("descriptor.bin"), &descriptor).unwrap();
        std::fs::write(dir.join("manifest-1.bin"), &manifest).unwrap();
        std::fs::write(dir.join("uuid.txt"), &uuid).unwrap();
    }
    // An index a restore has not finished, as a crash leaves it, for the same check.
    let marked = oss_format::mark_restoring(None).unwrap();
    let mut half = oss_format::IndexRecord::new("half", "uhalf", 2);
    half.settings = &marked;
    r.descriptors
        .create(&half, 1, &mut vec![0u8; 1 << 16])
        .unwrap();
    let held = read_register(&r, &oss_format::descriptor_key("", "half").unwrap());
    if std::env::var_os("OSS_WRITE_FIXTURES").is_some() {
        std::fs::write(dir.join("descriptor-marked.bin"), &held).unwrap();
    }
    assert!(std::fs::read(dir.join("descriptor-marked.bin")).is_ok());
    let committed =
        std::fs::read(dir.join("manifest-1.bin")).expect("run with OSS_WRITE_FIXTURES=1");
    let parsed = oss_format::CommitManifest::parse(&committed).expect("parses");
    assert_eq!(parsed.term, 1);
    assert!(std::fs::read(dir.join("descriptor.bin")).is_ok());
}

/// A finished shallow snapshot a Java node took, as `GenTake.java` wrote its
/// register value, restored by a Rust node: each file lands whole in the new
/// index's first term, whether the Java recorded it whole or packed.
#[test]
fn a_snapshot_a_java_node_took_is_restored_by_a_rust_node() {
    let r = rig(true);
    let body =
        r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups", Some(body)).status,
        200
    );
    let state = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/snapshots/java-take/state.bin"),
    )
    .expect("fixtures/snapshots/java-take/state.bin, written by GenTake.java");
    r.store
        .create_register(
            &oss_format::snapshot_state_key("", "backups", "java-1").unwrap(),
            &state,
        )
        .unwrap();
    let from = ShardRef {
        index: "logs",
        uuid: "uuidjavalogs000000000A",
        shard: 0,
    };
    let put = |key: &str, bytes: &[u8]| {
        r.store
            .put(key, &mut SliceSource::new(bytes), &mut [0u8; 64])
            .unwrap();
    };
    let (si, _) = from
        .blob_key("", "_0.si", &FileLocation::parse("t=3").unwrap())
        .unwrap();
    put(&si, b"0123456789");
    let (pack, offset) = from
        .blob_key("", "_0.cfs", &FileLocation::parse("t=3/pack#4:6").unwrap())
        .unwrap();
    assert_eq!(offset, 4);
    put(&pack, b"xxxxABCDEFyy");
    let rename = r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#;
    let reply = send(
        &r,
        "POST",
        "/_snapshot/backups/java-1/_restore?wait_for_completion=true",
        Some(rename),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    let uuid = restored_uuid(&r, "copy");
    let to = ShardRef {
        index: "copy",
        uuid: &uuid,
        shard: 0,
    };
    let whole = FileLocation::Whole { term_dir: "t=1" };
    let (si_to, _) = to.blob_key("", "_0.si", &whole).unwrap();
    let (cfs_to, _) = to.blob_key("", "_0.cfs", &whole).unwrap();
    assert_eq!(read(&r, &si_to), b"0123456789");
    assert_eq!(read(&r, &cfs_to), b"ABCDEF");
}

/// A rig that names `system` as system indices and holds the snapshot
/// `daily`, taken on a rig that named none.
fn rig_holding_a_snapshot(system: &str) -> Rig {
    let open = taken(true);
    let key = oss_format::snapshot_state_key("", "backups", "daily").unwrap();
    let mut buf = vec![0u8; 1 << 16];
    let n = open
        .store
        .read_register(&key, &mut buf)
        .unwrap()
        .unwrap()
        .len;
    let r = rig_with(true, system);
    let body =
        r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups", Some(body)).status,
        200
    );
    r.store.create_register(&key, &buf[..n]).unwrap();
    r
}

#[test]
fn a_system_index_is_not_restored() {
    let r = rig_holding_a_snapshot(".kibana*,logs");
    let rename = r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#;
    let reply = send(&r, "POST", RESTORE, Some(rename));
    assert_eq!(reply.status, 403, "{}", reply.text());
    assert!(reply.text().contains("system_index"), "{}", reply.text());
    assert!(
        r.descriptors
            .get("copy", &mut vec![0u8; 1 << 16])
            .unwrap()
            .is_none()
    );
}

#[test]
fn nothing_is_restored_into_a_system_index() {
    let r = rig_holding_a_snapshot(".kibana*");
    let rename = r#"{"rename_pattern":"logs","rename_replacement":".kibana_1"}"#;
    let reply = send(&r, "POST", RESTORE, Some(rename));
    assert_eq!(reply.status, 403, "{}", reply.text());
    assert!(
        r.descriptors
            .get(".kibana_1", &mut vec![0u8; 1 << 16])
            .unwrap()
            .is_none()
    );
}
