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

use super::{Gate, OpenPrefetch, Primed, Range};
use crate::shard_files::ShardFiles;
use crate::{OpenShard, open_located_with};

/// The default prefetch with a gate of its own, so tests do not share one.
fn fresh() -> OpenPrefetch {
    OpenPrefetch {
        gate: Gate::new(32 << 20, 32),
        ..OpenPrefetch::default()
    }
}

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
    rounds.reset(Duration::from_millis(20));
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
    let (_, depth, _) = cold_open(&store, "one", &fresh());
    assert_eq!(depth, 3, "{}", store.store().detail());
    let off = store_of(&Spec::new("one", 20));
    let (_, depth, _) = cold_open(&off, "one", &OpenPrefetch::off());
    assert!(depth > 3, "{}", off.store().detail());
}

#[test]
fn a_shard_of_several_segments_costs_no_more_requests_and_the_same_depth() {
    let spec = Spec::new("many", 40).segments_of(10);
    let on = store_of(&spec);
    let (with, depth, _) = cold_open(&on, "many", &fresh());
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
    let (_, _, shard) = cold_open(&store, "held", &fresh());
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
    let primed = Primed::fetch(&store, &[range("no/such/blob")], &Gate::new(1 << 20, 4));
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
    // Two buffers: the prefetch's concurrent ranges find the pool busy, so the
    // store refuses some; the engine reads those files itself.
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 2).expect("fits");
    let store = Arc::new(PooledStore::new(
        Rounds::new(Emulator::new(64 << 20, 1)),
        buffers,
    ));
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let (_, _, shard) = cold_open(&store, "tight", &fresh());
    assert_eq!(shard.reader.num_docs(), 40);
}

#[test]
fn what_an_open_took_from_the_gate_is_given_back_with_the_bytes() {
    let store = store_of(&Spec::new("gate", 20));
    let config = fresh();
    let (_, _, shard) = cold_open(&store, "gate", &config);
    assert_eq!(config.gate.held_bytes(), 0);
    assert_eq!(config.gate.held_threads(), 0);
    drop(shard);
}

#[test]
fn a_gate_with_no_room_leaves_every_range_to_the_engine() {
    let store = store_of(&Spec::new("full", 20));
    let config = OpenPrefetch {
        gate: Gate::new(0, 32),
        ..OpenPrefetch::default()
    };
    let (_, depth, shard) = cold_open(&store, "full", &config);
    assert!(depth > 3, "{}", store.store().detail());
    assert_eq!(shard.reader.num_docs(), 20);
    let config = OpenPrefetch {
        gate: Gate::new(32 << 20, 0),
        ..OpenPrefetch::default()
    };
    let other = store_of(&Spec::new("full", 20));
    let (_, depth, _) = cold_open(&other, "full", &config);
    assert!(depth > 3, "{}", other.store().detail());
}

#[test]
fn opens_at_once_never_hold_more_than_the_gate_and_a_refused_one_still_opens() {
    let store = store_of(&Spec::new("many", 40).segments_of(10));
    // Room for about one open's ranges: the second open of the pair is refused part.
    let gate = Gate::new(16 << 10, 2);
    let config = OpenPrefetch {
        gate: gate.clone(),
        ..OpenPrefetch::default()
    };
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    let (peak_bytes, peak_threads) = (AtomicU64::new(0), AtomicU64::new(0));
    let done = AtomicBool::new(false);
    std::thread::scope(|s| {
        // Samples what the gate holds while the opens fetch.
        s.spawn(|| {
            while !done.load(Ordering::Acquire) {
                peak_bytes.fetch_max(gate.held_bytes(), Ordering::Relaxed);
                peak_threads.fetch_max(gate.held_threads(), Ordering::Relaxed);
            }
        });
        std::thread::scope(|opens| {
            for _ in 0..4 {
                opens.spawn(|| {
                    let (_, _, shard) = cold_open(&store, "many", &config);
                    assert_eq!(shard.reader.num_docs(), 40);
                });
            }
        });
        done.store(true, Ordering::Release);
    });
    assert!(peak_bytes.load(Ordering::Relaxed) <= gate.max_bytes());
    assert!(peak_threads.load(Ordering::Relaxed) <= gate.max_threads());
    assert_eq!(gate.held_bytes(), 0);
    assert_eq!(gate.held_threads(), 0);
}

#[test]
fn a_cold_open_reads_the_same_requests_and_bytes_with_the_prefetch_as_without() {
    let shapes = [
        ("one", Spec::new("one", 20)),
        ("many", Spec::new("many", 40).segments_of(10)),
        (
            "wide",
            Spec::new("wide", 4000).segments_of(1000).padded(2000),
        ),
    ];
    for (name, spec) in shapes {
        let measure = |prefetch: &OpenPrefetch| {
            let store = store_of(&spec);
            let before = store.store().meter().bytes_read();
            let (requests, _, _) = cold_open(&store, name, prefetch);
            (requests, store.store().meter().bytes_read() - before)
        };
        let (on_requests, on_bytes) = measure(&fresh());
        let (off_requests, off_bytes) = measure(&OpenPrefetch::off());
        assert!(
            on_requests <= off_requests,
            "{name}: {on_requests} > {off_requests}"
        );
        assert_eq!(on_bytes, off_bytes, "{name}");
    }
}
