//! The open prefetch over a real index on the emulator: what a cold open
//! costs in requests and round trips, with it on and off.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;
use std::time::Duration;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, Emulator, PooledStore, Rounds, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use super::{OpenPrefetch, Primed, Range};
use crate::shard_files::ShardFiles;
use crate::{OpenShard, open_located_with};

type Store = Arc<PooledStore<Rounds<Emulator>>>;

fn store_of(spec: &Spec) -> Store {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 32).expect("fits");
    let store = Arc::new(PooledStore::new(
        Rounds::new(Emulator::new(64 << 20, 1)),
        buffers,
    ));
    for (key, bytes) in blobs("", spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    store
}

/// What opening `index` cold costs: requests, round trips, and the shard.
fn cold_open(
    store: &Store,
    index: &str,
    prefetch: &OpenPrefetch,
) -> (u64, usize, OpenShard<Rounds<Emulator>>) {
    let rounds = store.store();
    rounds.reset(Duration::from_millis(100));
    let before = rounds.meter().total();
    let (files, meta) = ShardFiles::open_with_meta(store, "", index, 0).expect("locates");
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    let shard = open_located_with(store.clone(), cache, "", index, 0, files, meta, prefetch)
        .expect("opens");
    (rounds.meter().total() - before, rounds.depth(), shard)
}

#[test]
fn a_cold_open_reads_its_files_in_one_round_after_the_two_registers() {
    let store = store_of(&Spec::new("one", 20));
    let (_, depth, _) = cold_open(&store, "one", &OpenPrefetch::default());
    assert_eq!(depth, 3, "{}", store.store().detail());
    let off = store_of(&Spec::new("one", 20));
    let (_, depth, _) = cold_open(&off, "one", &OpenPrefetch::off());
    assert!(depth > 3, "{}", off.store().detail());
}

#[test]
fn a_shard_of_several_segments_costs_no_more_requests_and_the_same_depth() {
    let spec = Spec::new("many", 40).segments_of(10);
    let on = store_of(&spec);
    let (with, depth, _) = cold_open(&on, "many", &OpenPrefetch::default());
    let off = store_of(&spec);
    let (without, _, _) = cold_open(&off, "many", &OpenPrefetch::off());
    assert!(
        with <= without,
        "{with} > {without}\n{}",
        on.store().detail()
    );
    assert_eq!(depth, 3, "{}", on.store().detail());
}

#[test]
fn what_was_fetched_is_released_when_the_open_returns() {
    let store = store_of(&Spec::new("held", 20));
    let (_, _, shard) = cold_open(&store, "held", &OpenPrefetch::default());
    assert!(!shard.directory.store().is_primed());
}

#[test]
fn a_range_the_store_refuses_is_left_to_the_engine() {
    let store = store_of(&Spec::new("gone", 20));
    let range = |key: &str| Range {
        key: key.to_owned(),
        start: 0,
        len: 4,
        members: vec![super::Member {
            name: "f".to_owned(),
            at: 0,
            len: 4,
        }],
    };
    let primed = Primed::fetch(&store, &[range("no/such/blob")]);
    assert!(primed.is_empty());
    let mut out = [0u8; 4];
    assert_eq!(primed.read("f", 0, &mut out), None);
}

#[test]
fn an_open_whose_prefetch_is_refused_still_opens_and_reads_every_document() {
    let spec = Spec::new("tight", 40);
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    // Two buffers: the prefetch's four concurrent ranges find the pool busy.
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 2).expect("fits");
    let store = Arc::new(PooledStore::new(
        Rounds::new(Emulator::new(64 << 20, 1)),
        buffers,
    ));
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let (_, _, shard) = cold_open(&store, "tight", &OpenPrefetch::default());
    assert_eq!(shard.reader.num_docs(), 40);
}

struct NoIndex;

impl oss_blob::usage::Classifier for NoIndex {
    fn index_of<'a>(&self, _key: &'a str) -> Option<&'a str> {
        None
    }
}

// docs/specs/request-cost-header AC-6
#[test]
fn the_reads_of_the_prefetch_threads_count_in_the_request_that_asked() {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let ledger = Arc::new(oss_blob::usage::Ledger::new(8));
    let store = PooledStore::new(
        oss_blob::UsageStore::new(Emulator::new(1 << 20, 1), ledger, Arc::new(NoIndex)),
        buffers,
    );
    store
        .put("a/blob", &mut SliceSource::new(&[7u8; 64]))
        .expect("put");
    let range = |start: u64| Range {
        key: "a/blob".to_owned(),
        start,
        len: 4,
        members: vec![super::Member {
            name: format!("f{start}"),
            at: 0,
            len: 4,
        }],
    };
    let cost = oss_blob::cost::RequestCost::new();
    let primed = {
        let _scope = oss_blob::cost::enter(Some(Arc::clone(&cost)));
        Primed::fetch(&store, &[range(0), range(8), range(16)])
    };
    assert!(!primed.is_empty());
    assert_eq!(cost.requests(oss_blob::S3Op::GetRange), 3);
    // Without a scope on the calling thread the same reads count nowhere.
    let other = oss_blob::cost::RequestCost::new();
    let _ = Primed::fetch(&store, &[range(0)]);
    assert_eq!(other.requests(oss_blob::S3Op::GetRange), 0);
}
