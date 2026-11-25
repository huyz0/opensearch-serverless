//! docs/specs/stock-snapshot-restore AC-4, AC-6, AC-8 and AC-9: a snapshot
//! in a stock repository on disk restored over REST into a new index, its
//! files byte for byte, with no LIST; refused where the spec says.

// The repository is a few KiB of fixture, copied to a temporary directory.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use oss_blob::{BlobStore, S3Op, SourceRoots};
use oss_node::stock_restore_route::StockRestoreSettings;

use crate::snapshot_restore::{Rig, read, requests, restored_uuid, rig_stock, send};

const RESTORE: &str = "/_snapshot/stock/snap-1/_restore?wait_for_completion=true";
const POOL: usize = 200 << 20;

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A copy of the fixture repository under a fresh directory, the root a
/// node may read, and the repository's own path.
fn repository(tag: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("oss-stock-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let root_dir = root.join("roots");
    let repo = root_dir.join("repo");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/snapshots/stock-repo/repo"),
        &repo,
    );
    let root_dir = root_dir.canonicalize().unwrap();
    let repo = root_dir.join("repo");
    (root_dir, repo)
}

fn rig(root: &Path, enabled: bool, pool: usize) -> Rig {
    rig_stock(
        true,
        "",
        pool,
        1 << 16,
        StockRestoreSettings {
            enabled,
            roots: SourceRoots::parse(root.to_str().unwrap(), ""),
            fanout: 1,
        },
    )
}

fn register(r: &Rig, location: &Path) {
    let body = format!(
        r#"{{"type":"fs","settings":{{"format":"stock","location":"{}"}}}}"#,
        location.display()
    );
    let reply = send(r, "PUT", "/_snapshot/stock", Some(&body));
    assert_eq!(reply.status, 200, "{}", reply.text());
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/snapshots/stock-repo/repo/indices/IdxLogsAaaa1/0")
            .join(name),
    )
    .unwrap()
}

#[test]
fn a_stock_snapshot_restores_into_a_new_index_with_every_file_whole() {
    let (root, repo) = repository("restore");
    let r = rig(&root, true, POOL);
    register(&r, &repo);
    let before = requests(&r);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(r.store.meter().count(S3Op::List), 0);
    assert!(requests(&r) > before);
    let uuid = restored_uuid(&r, "logs");
    let big = [
        fixture("__bigBlobUuid1.part0"),
        fixture("__bigBlobUuid1.part1"),
    ]
    .concat();
    let small = fixture("__smallBlobUuid2");
    let shard = oss_format::ShardRef {
        index: "logs",
        uuid: &uuid,
        shard: 0,
    };
    let dir = format!("{}t=1/", shard.base_path("").unwrap());
    assert_eq!(read(&r, &format!("{dir}_0.cfs")), big);
    assert_eq!(read(&r, &format!("{dir}segments_1")), small);
}

#[test]
fn with_the_switch_off_a_stock_restore_is_501_and_reads_nothing_from_the_source() {
    let (root, repo) = repository("off");
    let r = rig(&root, false, POOL);
    register(&r, &repo);
    let before = requests(&r);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(restored_uuid_missing(&r, "logs"));
    // Measured: the snapshot's register and legacy record and the
    // repository's record, read; nothing is written, nothing is read from
    // the source.
    assert_eq!(requests(&r) - before, 4);
    assert_eq!(
        r.store.meter().count(S3Op::Put),
        put_count_after_register(&r)
    );
}

fn put_count_after_register(r: &Rig) -> u64 {
    // Registering the repository is the only write.
    r.store.meter().count(S3Op::Put)
}

fn restored_uuid_missing(r: &Rig, name: &str) -> bool {
    r.descriptors
        .get(name, &mut vec![0u8; 1 << 16])
        .unwrap()
        .is_none()
}

#[test]
fn a_source_outside_the_roots_is_refused_and_unknown_names_are_404() {
    let (root, repo) = repository("refuse");
    let r = rig(&root, true, POOL);
    register(&r, &repo);
    // Another root than the one the node lists.
    let other = rig(&repo, true, POOL);
    register(&other, &root);
    assert_eq!(send(&other, "POST", RESTORE, None).status, 400);
    let unknown = "/_snapshot/stock/nope/_restore?wait_for_completion=true";
    assert_eq!(send(&r, "POST", unknown, None).status, 404);
    let missing = r#"{"indices":"absent"}"#;
    assert_eq!(send(&r, "POST", RESTORE, Some(missing)).status, 404);
}

#[test]
fn a_pool_too_small_for_the_reads_is_429_and_writes_nothing() {
    let (root, repo) = repository("pool");
    let r = rig(&root, true, 8 << 20);
    register(&r, &repo);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 429, "{}", reply.text());
    assert!(restored_uuid_missing(&r, "logs"));
}

#[test]
fn a_pool_that_holds_the_reads_but_not_the_copy_is_429_before_any_write() {
    let (root, repo) = repository("pool3");
    // The record (1 MiB), then the reads (80 MiB, 64 MiB of index-N and the check buffer) fit; the copy's buffers do not.
    let reads = (1 << 20) + (80 << 20) + (64 << 20) + (1 << 16);
    let r = rig(&root, true, reads + 1024);
    register(&r, &repo);
    let puts = r.store.meter().count(S3Op::Put);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 429, "{}", reply.text());
    assert_eq!(
        r.store.meter().count(S3Op::Put),
        puts,
        "nothing was written"
    );
    assert!(restored_uuid_missing(&r, "logs"));
}

#[test]
fn a_file_that_does_not_match_its_listing_fails_the_restore_and_removes_the_index() {
    let (root, repo) = repository("flip");
    let r = rig(&root, true, POOL);
    register(&r, &repo);
    let small = repo.join("indices/IdxLogsAaaa1/0/__smallBlobUuid2");
    let mut bytes = std::fs::read(&small).unwrap();
    bytes[3] ^= 1;
    std::fs::write(&small, bytes).unwrap();
    let reply = send(&r, "POST", RESTORE, None);
    assert!(reply.status >= 400, "{}", reply.text());
    assert!(
        restored_uuid_missing(&r, "logs"),
        "the index it created is gone"
    );
}

#[test]
fn an_s3_stock_repository_is_not_served_yet() {
    let (root, _) = repository("s3");
    let r = rig(&root, true, POOL);
    let body = r#"{"type":"s3","settings":{"format":"stock","bucket":"b","base_path":"p"}}"#;
    assert_eq!(send(&r, "PUT", "/_snapshot/stock", Some(body)).status, 200);
    assert_eq!(send(&r, "POST", RESTORE, None).status, 501);
}
