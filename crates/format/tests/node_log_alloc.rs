//! Encoding a window allocates the blob once, at its exact length, and
//! copies each record once (docs/specs/node-log-format/SPEC.md, AC-5). Its
//! own binary, because the counting allocator is process-wide.

// ARITH: sizes here are a test's own; an overflow panics the test in a
// debug build, which fails it.
#![allow(clippy::arithmetic_side_effects)]

use oss_format::node_log_format::encode;
use oss_format::{OrdinalRange, ShardBatch, ShardKey, WalRecord};
use oss_testkit::{CountingAllocator, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

#[test]
fn encode_allocates_the_blob_once() {
    let source = format!("{{\"text\":\"{}\"}}", "x".repeat(64 * 1024));
    let shards: Vec<ShardKey<'static>> = (0..16)
        .map(|s| ShardKey::new("index-uuid", s, Some("logs")))
        .collect();
    let records: Vec<Vec<_>> = shards
        .iter()
        .map(|_| {
            (0..4)
                .map(|i| {
                    WalRecord::index(i.to_string(), source.as_str(), i, 1, 1)
                        .encode()
                        .unwrap()
                })
                .collect()
        })
        .collect();
    let batches: Vec<ShardBatch<'_>> = shards
        .iter()
        .zip(&records)
        .map(|(shard, records)| ShardBatch {
            shard,
            term: 1,
            records,
        })
        .collect();
    let prior: Vec<_> = (16..64)
        .map(|s| {
            (
                ShardKey::new("other", s, Some("old")),
                vec![OrdinalRange { from: 2, to: 3 }],
            )
        })
        .collect();

    let before = ALLOC.total_allocated();
    let blob = encode("node-1", 1, 9, 2, &batches, &prior).unwrap();
    let allocated = ALLOC.total_allocated() - before;
    let len = blob.bytes.len();
    assert!(len > 4 * 1024 * 1024, "{len}");
    assert_eq!(
        blob.bytes.capacity(),
        len,
        "the blob grew past its measured length"
    );
    // The blob once, plus the header's own entries: far less than a second copy.
    assert!(
        allocated < len + 64 * 1024,
        "allocated {allocated} for a blob of {len}"
    );
}
