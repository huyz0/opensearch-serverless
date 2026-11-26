//! docs/specs/snapshot-take AC-7: a take costs the same number of requests
//! for a kilobyte index as for one of a hundred megabytes, because no request
//! reads a data file.

// ARITH: one request count subtracted from a later one; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use crate::snapshot_take::{TAKE, index, repository, requests, rig, send};

use oss_blob::{BlobStore, SliceSource};
use oss_format::{FileLocation, ManifestEntry, ShardRef, encode_manifest};

/// Publishes one shard whose one file holds `bytes`, written to the store.
fn publish_sized(r: &crate::snapshot_take::Rig, index: &str, uuid: &str, bytes: &[u8]) {
    let at = ShardRef {
        index,
        uuid,
        shard: 0,
    };
    let key = format!("{}t=1/_0.si", at.base_path("").unwrap());
    r.store
        .put(&key, &mut SliceSource::new(bytes), &mut [0u8; 64])
        .unwrap();
    let entries = [ManifestEntry {
        name: "_0.si",
        location: FileLocation::Whole { term_dir: "t=1" }.encode(),
        length: bytes.len() as u64,
    }];
    let value = encode_manifest(1, Some("n1"), &entries, -1, -1).unwrap();
    r.store
        .create_register(&at.manifest_key("").unwrap(), &value)
        .unwrap();
}

fn cost_of_a_take(bytes: &[u8]) -> u64 {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish_sized(&r, "logs", "u1", bytes);
    let before = requests(&r);
    let made = send(&r, "PUT", "/_snapshot/backups/daily", Some(TAKE));
    assert_eq!(made.status, 200, "{}", made.text());
    requests(&r) - before
}

#[test]
fn a_take_of_a_hundred_megabyte_index_costs_what_a_kilobyte_one_does() {
    let small = cost_of_a_take(&[7u8; 1 << 10]);
    let large = cost_of_a_take(&vec![7u8; 100 << 20]);
    assert!(small > 0, "the take made requests");
    assert_eq!(small, large);
}
