//! The plan over a hand-built file table.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants.
#![allow(clippy::arithmetic_side_effects)]

use std::collections::BTreeMap;

use super::{OpenPrefetch, plan};
use crate::shard_files::{FileRange, ShardFiles};

fn table(files: &[(&str, &str, u64, u64)]) -> ShardFiles {
    let map: BTreeMap<String, FileRange> = files
        .iter()
        .map(|&(name, key, offset, length)| {
            (
                name.to_owned(),
                FileRange {
                    key: key.to_owned(),
                    offset,
                    length,
                },
            )
        })
        .collect();
    ShardFiles::of(1, map)
}

fn pin_si(name: &str) -> bool {
    name.ends_with(".si")
}

#[test]
fn neighbours_of_one_blob_share_a_range_and_blobs_never_do() {
    let files = table(&[
        ("_0.si", "a", 0, 100),
        ("_0.fnm", "a", 100, 50),
        ("_1.fnm", "b", 0, 50),
    ]);
    let ranges = plan(&files, &pin_si, 4096, &OpenPrefetch::default());
    assert_eq!(ranges.len(), 2);
    let a = &ranges[0];
    assert_eq!((a.key.as_str(), a.start, a.len), ("a", 0, 150));
    assert_eq!(a.members.len(), 2);
    assert_eq!(ranges[1].key, "b");
}

#[test]
fn a_gap_wider_than_the_limit_splits_a_range() {
    let files = table(&[("_0.fnm", "a", 0, 10), ("_1.fnm", "a", 1000, 10)]);
    let config = OpenPrefetch {
        gap_bytes: 100,
        ..OpenPrefetch::default()
    };
    assert_eq!(plan(&files, &pin_si, 4096, &config).len(), 2);
    let config = OpenPrefetch {
        gap_bytes: 2000,
        ..config
    };
    let merged = plan(&files, &pin_si, 4096, &config);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].len, 1010);
}

#[test]
fn a_large_unpinned_file_is_left_to_the_engine_and_a_pinned_one_is_not() {
    let files = table(&[
        ("_0.doc", "a", 0, 1 << 20),
        ("_0.si", "a", 1 << 20, 1 << 20),
        ("_0.fnm", "a", 2 << 20, 10),
    ]);
    let config = OpenPrefetch {
        max_bytes: u64::MAX,
        gap_bytes: 0,
        ..OpenPrefetch::default()
    };
    let names: Vec<_> = plan(&files, &pin_si, 4096, &config)
        .iter()
        .flat_map(|r| r.members.iter().map(|m| m.name.clone()))
        .collect();
    assert_eq!(names, ["_0.si", "_0.fnm"]);
}

#[test]
fn the_plan_stops_at_the_byte_limit() {
    let files = table(&[
        ("_0.fnm", "a", 0, 60),
        ("_1.fnm", "a", 60, 60),
        ("_2.fnm", "a", 120, 60),
    ]);
    let config = OpenPrefetch {
        max_bytes: 130,
        ..OpenPrefetch::default()
    };
    let picked: u64 = plan(&files, &pin_si, 4096, &config)
        .iter()
        .flat_map(|r| &r.members)
        .map(|m| m.len)
        .sum();
    assert_eq!(picked, 120);
}

#[test]
fn an_empty_file_table_plans_nothing() {
    assert!(plan(&table(&[]), &pin_si, 4096, &OpenPrefetch::default()).is_empty());
}
