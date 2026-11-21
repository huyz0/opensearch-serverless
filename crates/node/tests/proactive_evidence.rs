//! docs/specs/proactive-takeover-faults: what switching the proactive pass on
//! costs. The request counts are gates; the latency run is evidence, run by
//! hand (`--ignored --nocapture`), because a percentile is not a predicate.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_blob::BlobStore;
use oss_blob::S3Op;
use oss_blob::emulator::faults::Latency;
use oss_node::writer::ProactiveTakeover;
use oss_testkit::budget;
use oss_writer::takeover::Predecessors;
use support::*;

fn total(c: &Cluster) -> u64 {
    c.store.meter().total()
}

/// node-1 has written one document to each of four shards and died; node-2
/// takes over, `proactively` or by the first write to each shard. Returns
/// the requests that took, and the lists among them.
fn requests_to_serve_every_shard(proactively: bool) -> (u64, u64) {
    let c = cluster();
    create_n(&c, "logs", "u1", 4);
    let ids = ids_by_shard(4, 1);
    let one = node(&c, "node-1", 1, false, None);
    for ids in &ids {
        write_id(&one.role, &ids[0]);
    }
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors: Arc<Predecessors> = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    let before = (total(&c), c.store.meter().count(S3Op::List));
    if proactively {
        let pass =
            ProactiveTakeover::new("node-2", config(true), Arc::clone(&two.role), predecessors);
        let report = pass.pass(&dead(&one));
        assert_eq!(report.taken, 4, "{report:?}");
    }
    for ids in &ids {
        write_id(&two.role, &ids[0]);
    }
    let after = (total(&c), c.store.meter().count(S3Op::List));
    for op in [S3Op::Get, S3Op::GetRange, S3Op::Head, S3Op::Put, S3Op::List] {
        eprintln!(
            "proactive={proactively} {op:?} {}",
            c.store.meter().count(op)
        );
    }
    (after.0 - before.0, after.1 - before.1)
}

#[test]
fn the_pass_costs_one_request_a_death_over_the_writes_that_would_have_taken_the_shards() {
    let (lazy, lazy_lists) = requests_to_serve_every_shard(false);
    let (proactive, lists) = requests_to_serve_every_shard(true);
    eprintln!("requests to serve four shards: lazy {lazy}, proactive {proactive}");
    assert_eq!((lazy_lists, lists), (0, 0), "no LIST");
    // The one extra is the pass's read of the index's descriptor; it does
    // not grow with the shards (2, 4 and 6 shards measured the same +1).
    assert!(proactive <= lazy + 1, "proactive {proactive}, lazy {lazy}");
}

#[test]
fn with_the_switch_on_a_fleet_with_no_death_makes_no_request() {
    let c = cluster();
    let one = first_writer(&c);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    let pass = Arc::new(ProactiveTakeover::new(
        "node-2",
        config(true),
        Arc::clone(&two.role),
        predecessors,
    ));
    let before = total(&c);
    // Nobody dead, then a dead node whose lease names no epochs.
    let mut lease = dead(&one);
    lease.node_log.clear();
    for dead_with_epochs in [vec![], vec![lease]] {
        let mut r = refreshed(&["node-2", "node-3"], dead(&one));
        r.dead_with_epochs = dead_with_epochs;
        for _ in 0..50 {
            pass.on_refresh(&r, i64::MAX);
        }
    }
    assert_eq!(total(&c), before);
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[(((sorted.len() - 1) as f64) * p) as usize]
}

/// Writes by a live node to its own index, with `pass` running or not on
/// another node over a dead node's four shards, at S3-like latency.
fn write_latencies(pass_running: bool) -> Vec<Duration> {
    let c = cluster();
    create_n(&c, "logs", "u1", 4);
    create_n(&c, "other", "u9", 1);
    let one = node(&c, "node-1", 1, false, None);
    for ids in ids_by_shard(4, 1) {
        write_id(&one.role, &ids[0]);
    }
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    let three = node(&c, "node-3", 3, false, None);
    write_in(&three.role, "other", "warm");
    for (op, base, jitter) in [
        (S3Op::Get, 20, 10),
        (S3Op::GetRange, 20, 10),
        (S3Op::Head, 20, 10),
        (S3Op::Put, 40, 20),
    ] {
        c.store.set_latency(
            op,
            Latency {
                base: Duration::from_millis(base),
                jitter: Duration::from_millis(jitter),
            },
        );
    }
    let pass = ProactiveTakeover::new("node-2", config(true), Arc::clone(&two.role), predecessors);
    let lease = dead(&one);
    std::thread::scope(|s| {
        if pass_running {
            s.spawn(|| pass.pass(&lease));
        }
        (0..300)
            .map(|n| {
                let start = Instant::now();
                write_in(&three.role, "other", &format!("w{n}"));
                start.elapsed()
            })
            .collect()
    })
}

#[test]
#[ignore = "evidence for the switch's default: run by hand with --nocapture"]
fn write_latency_of_a_live_node_while_a_pass_runs() {
    let mut rows = Vec::new();
    for running in [false, true] {
        let mut all: Vec<Duration> = (0..5).flat_map(|_| write_latencies(running)).collect();
        all.sort();
        rows.push((percentile(&all, 0.5), percentile(&all, 0.99)));
        eprintln!(
            "pass running {running}: p50 {:?} p99 {:?} ({} writes)",
            rows.last().unwrap().0,
            rows.last().unwrap().1,
            all.len()
        );
    }
    let ratio = rows[1].1.as_secs_f64() / rows[0].1.as_secs_f64();
    eprintln!("p99 ratio on/off {ratio:.3}");
    assert!(ratio <= 1.05, "write p99 {ratio:.3}x with the pass running");
}

/// The requests of one pass over a dead node's log naming `shards` unpublished
/// shards: the fence, the header, a descriptor read, and each take.
fn pass_requests(shards: u32) -> (u64, u64) {
    let c = cluster();
    create_n(&c, "logs", "u1", shards);
    let one = node(&c, "node-1", 1, false, None);
    for ids in ids_by_shard(shards, 1) {
        write_id(&one.role, &ids[0]);
    }
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    let pass = ProactiveTakeover::new("node-2", config(true), Arc::clone(&two.role), predecessors);
    let before = (total(&c), c.store.meter().count(S3Op::List));
    let report = pass.pass(&dead(&one));
    assert_eq!(report.taken, shards as usize, "{report:?}");
    (
        total(&c) - before.0,
        c.store.meter().count(S3Op::List) - before.1,
    )
}

#[test]
fn a_pass_costs_what_the_table_says() {
    let (one, lists_one) = pass_requests(1);
    let (four, lists_four) = pass_requests(4);
    budget::check(
        "takeover.proactive.one_shard.requests",
        one,
        "a pass over one named shard",
    );
    budget::check(
        "takeover.proactive.four_shards.requests",
        four,
        "a pass over four named shards",
    );
    budget::check(
        "takeover.proactive.lists",
        lists_one + lists_four,
        "LISTs of both passes",
    );
}

/// What a reader opens of shard 0 of `logs` now: its live documents.
fn documents_a_reader_sees(c: &Cluster) -> u64 {
    use lucene_store::page_cache::{PageCache, PageCacheConfig};
    use oss_blob::PooledStore;
    use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
    let config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 4).expect("fits");
    let store = Arc::new(PooledStore::new(
        oss_blob::Shared::new(c.store.clone()),
        buffers,
    ));
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("cache"));
    let open = oss_reader::open_shard(store, cache, "", "logs", 0).expect("opens");
    let live = open
        .reader
        .soft_deletes_wrapper("__soft_deletes")
        .expect("wrapped");
    u64::try_from(live.num_docs()).expect("count")
}

#[test]
fn a_reader_sees_the_same_documents_after_a_pass_as_after_a_write_takes_the_shard() {
    let mut seen = Vec::new();
    for proactively in [false, true] {
        let c = cluster();
        let one = first_writer(&c);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let take = takeover(&c, &one, &calls);
        let predecessors = Arc::clone(&take.predecessors);
        let two = node(&c, "node-2", 2, true, Some(take));
        if proactively {
            let pass =
                ProactiveTakeover::new("node-2", config(true), Arc::clone(&two.role), predecessors);
            assert_eq!(pass.pass(&dead(&one)).taken, 1);
        } else {
            write_id(&two.role, "c");
            two.role.refresh(&info(), 0).expect("published");
        }
        seen.push((registered_term(&c), documents_a_reader_sees(&c)));
    }
    // a was published by the dead node, b only logged: both are searchable once
    // the shard is taken, and the pass publishes without a write to ask for it.
    assert_eq!(seen[1], (2, 2), "{seen:?}");
    assert_eq!(seen[0].0, seen[1].0);
    assert!(seen[0].1 >= seen[1].1, "{seen:?}");
}
