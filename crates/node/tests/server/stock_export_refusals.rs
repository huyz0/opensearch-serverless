//! docs/specs/stock-snapshot-export AC-7: a delete of a snapshot in a stock
//! repository, and a take of a file above the store's single-object limit,
//! are refused 501 and change nothing.

#![allow(clippy::disallowed_methods)]

use oss_blob::BlobStore;
use oss_format::{FileLocation, ManifestEntry, ShardRef, encode_manifest};

use crate::stock_export::{TAKE, WAIT, rig, send, shard_files};

#[test]
fn a_delete_of_a_stock_snapshot_is_501_and_the_repository_is_untouched() {
    let r = rig(true, "delete");
    let taken = send(&r, "PUT", &format!("/_snapshot/stock/s1{WAIT}"), Some(TAKE));
    assert_eq!(taken.status, 200, "{}", taken.text());
    let before = r.written();
    let reply = send(&r, "DELETE", "/_snapshot/stock/s1", None);
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(reply.text().contains("stock"), "{}", reply.text());
    assert_eq!(r.written(), before, "nothing was removed or written");
    // A name the repository does not hold is the same refusal, not a 404 that
    // says the snapshot may be missing from a repository this node cannot read.
    let other = send(&r, "DELETE", "/_snapshot/stock/never", None);
    assert_eq!(other.status, 501, "{}", other.text());
}

/// The most one stored object may hold: S3's single put, 5 GiB.
const MAX_OBJECT: u64 = 5 << 30;

/// A shard whose manifest lists one more file of `length` bytes; the file is
/// never read, because the take refuses on the listed length before any copy.
fn list_file_of(r: &crate::stock_export::Rig, length: u64) {
    let key = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    }
    .manifest_key("")
    .unwrap();
    let mut buf = vec![0u8; 1 << 16];
    let read = r.store.read_register(&key, &mut buf).unwrap().unwrap();
    let files = shard_files();
    let location = FileLocation::Whole { term_dir: "t=1" }.encode();
    let mut entries: Vec<ManifestEntry<'_>> = files
        .iter()
        .map(|(name, bytes)| ManifestEntry {
            name,
            location: location.clone(),
            length: bytes.len() as u64,
        })
        .collect();
    entries.push(ManifestEntry {
        name: "_9.cfs",
        location,
        length,
    });
    let value = encode_manifest(1, Some("n1"), &entries, -1, -1).unwrap();
    let swapped = r
        .store
        .swap_register(&key, read.generation, &value)
        .unwrap();
    assert!(
        matches!(swapped, oss_blob::SwapOutcome::Applied(_)),
        "{swapped:?}"
    );
}

#[test]
fn a_file_above_the_single_object_limit_is_501_and_nothing_is_written() {
    let r = rig(true, "oversize");
    list_file_of(&r, MAX_OBJECT + 1);
    let reply = send(&r, "PUT", &format!("/_snapshot/stock/s1{WAIT}"), Some(TAKE));
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(r.written().is_empty(), "nothing was written");
}

#[test]
fn a_file_of_exactly_the_single_object_limit_is_not_refused_for_its_size() {
    let r = rig(true, "at-limit");
    list_file_of(&r, MAX_OBJECT);
    let reply = send(&r, "PUT", &format!("/_snapshot/stock/s1{WAIT}"), Some(TAKE));
    // The listed file does not exist, so the take fails when it reads it, but not as a size refusal.
    assert_ne!(reply.status, 501, "{}", reply.text());
}
