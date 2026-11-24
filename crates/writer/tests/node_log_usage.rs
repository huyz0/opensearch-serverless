//! A log blob's PUT is shared among the indices in it by the bytes of their
//! slices (docs/specs/store-usage-log/SPEC.md).

// ARITH: counts and shares here are small test constants.
#![allow(clippy::arithmetic_side_effects)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{record, window};
use oss_blob::usage::{Classifier, Ledger, NODE, ONE_REQUEST};
use oss_blob::{BlobStore, Emulator, S3Op, UsageStore};
use oss_format::{ShardBatch, ShardKey};
use oss_writer::log::{NodeLogStore, ThreadPerPut};

/// No key of the log names an index.
struct Unowned;

impl Classifier for Unowned {
    fn index_of<'a>(&self, _key: &'a str) -> Option<&'a str> {
        None
    }
}

fn rig() -> (Arc<UsageStore<Emulator>>, Arc<Ledger>, NodeLogStore) {
    let ledger = Arc::new(Ledger::new(8));
    let store = Arc::new(UsageStore::new(
        Emulator::new(1 << 24, 7),
        Arc::clone(&ledger),
        Arc::new(Unowned),
    ));
    let log = NodeLogStore::new(Arc::clone(&store) as Arc<dyn BlobStore>, "", "node-1")
        .expect("names")
        .with_usage(Arc::clone(&ledger));
    (store, ledger, log)
}

/// A window: three records of index `a` on one shard and one of `b`.
fn two_indices() -> oss_format::EncodedNodeLog {
    let a = ShardKey::new("uuid-a", 0, Some("a"));
    let b = ShardKey::new("uuid-b", 0, Some("b"));
    let ra = [record("1", 1), record("2", 2), record("3", 3)];
    let rb = [record("4", 4)];
    window(
        0,
        &[
            ShardBatch {
                shard: &a,
                term: 1,
                records: &ra,
            },
            ShardBatch {
                shard: &b,
                term: 1,
                records: &rb,
            },
        ],
    )
}

fn check(ledger: &Ledger, store: &UsageStore<Emulator>, encoded: &oss_format::EncodedNodeLog) {
    let (a, b) = (ledger.of("a"), ledger.of("b"));
    let (la, lb) = (
        u64::from(encoded.header.slices[0].length),
        u64::from(encoded.header.slices[1].length),
    );
    assert!(la > lb);
    assert_eq!(a.requests(S3Op::Put) + b.requests(S3Op::Put), ONE_REQUEST);
    // The lighter index takes its rounded-down share; the heavier, the rest.
    assert_eq!(b.requests(S3Op::Put), ONE_REQUEST * lb / (la + lb));
    assert!(a.requests(S3Op::Put) > b.requests(S3Op::Put));
    assert_eq!(ledger.of(NODE).requests(S3Op::Put), 0);
    assert_eq!(
        a.bytes_written() + b.bytes_written(),
        store.meter().bytes_written()
    );
}

// AC-1
#[test]
fn a_blobs_put_is_shared_by_the_bytes_of_each_indexs_slices() {
    let (store, ledger, log) = rig();
    let encoded = two_indices();
    log.append(3, 0, &encoded.bytes).expect("lands");
    assert_eq!(store.meter().count(S3Op::Put), 1);
    check(&ledger, &store, &encoded);
}

// AC-2
#[test]
fn a_hedged_put_is_shared_too_though_it_runs_on_another_thread() {
    let (store, ledger, log) = rig();
    let encoded = two_indices();
    let blob: Arc<[u8]> = Arc::from(&encoded.bytes[..]);
    log.append_hedged(3, 0, &blob, Duration::from_secs(30), Some(&ThreadPerPut))
        .expect("lands");
    assert_eq!(store.meter().count(S3Op::Put), 1);
    check(&ledger, &store, &encoded);
}

// AC-3
#[test]
fn a_blob_whose_slices_name_no_index_is_the_nodes() {
    let (_store, ledger, log) = rig();
    let nameless = ShardKey::new("uuid-c", 0, None::<&str>);
    let records = [record("1", 1)];
    let encoded = window(
        0,
        &[ShardBatch {
            shard: &nameless,
            term: 1,
            records: &records,
        }],
    );
    log.append(3, 0, &encoded.bytes).expect("lands");
    assert_eq!(ledger.of(NODE).requests(S3Op::Put), ONE_REQUEST);
}

// AC-4
#[test]
fn a_log_without_usage_charges_nothing_to_an_index() {
    let ledger = Arc::new(Ledger::new(8));
    let store = Arc::new(UsageStore::new(
        Emulator::new(1 << 24, 7),
        Arc::clone(&ledger),
        Arc::new(Unowned),
    ));
    let log =
        NodeLogStore::new(Arc::clone(&store) as Arc<dyn BlobStore>, "", "node-1").expect("names");
    log.append(3, 0, &two_indices().bytes).expect("lands");
    assert_eq!(ledger.of(NODE).requests(S3Op::Put), ONE_REQUEST);
    assert_eq!(ledger.of("a").requests(S3Op::Put), 0);
}
