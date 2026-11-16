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

/// The allocator counts every thread: tests that measure it take turns.
static MEASURING: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn encode_allocates_the_blob_once() {
    let _turn = MEASURING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let source = format!("{{\"text\":\"{}\"}}", "x".repeat(64 * 1024));
    let shards: Vec<ShardKey> = (0..16)
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

#[test]
fn decode_shares_each_name_across_its_entries() {
    let _turn = MEASURING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // One index whose uuid is not plain UTF-8 (it holds a NUL), so decoding
    // it makes an owned copy: 50,000 entries name it, and must share that
    // copy rather than make one each.
    let uuid = format!("{}\u{0}", "u".repeat(4_000));
    let prior: Vec<_> = (0..50_000)
        .map(|s| {
            (
                ShardKey::new(uuid.as_str(), s, None::<&str>),
                vec![OrdinalRange { from: 1, to: 1 }],
            )
        })
        .collect();
    let blob = encode("n", 0, 2, 0, &[], &prior).unwrap();
    let header_bytes = blob.bytes.len();

    let before = ALLOC.total_allocated();
    let started = std::time::Instant::now();
    let entry = oss_format::node_log_format::decode(&blob.bytes).unwrap();
    let elapsed = started.elapsed();
    let allocated = ALLOC.total_allocated() - before;
    let oss_format::NodeLogEntry::Records(header) = entry else {
        panic!("not records")
    };
    assert_eq!(header.index.len(), 50_000);
    // Entries and runs, a few words each: well under a copy of the uuid each,
    // which would be 200 MB.
    assert!(
        allocated < 8 * header_bytes,
        "allocated {allocated} for a header of {header_bytes}"
    );
    // Linear, not quadratic: a debug build decodes this in milliseconds.
    assert!(elapsed.as_secs() < 5, "{elapsed:?}");
}
