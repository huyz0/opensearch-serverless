//! docs/specs/takeover-scale: one dead node's 1,000 shards taken by the
//! proactive pass, timed at the Java soak's request latency. Evidence run by
//! hand: `cargo test -p oss-node --features writer --release --test
//! takeover_scale -- --ignored --nocapture`.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

mod proactive_support;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

use oss_blob::emulator::faults::Latency;
use oss_blob::{BlobStore, S3Op};
use oss_node::writer::ProactiveTakeover;
use proactive_support::*;

const SHARDS: u32 = 1_000;
const DOCS: u32 = 3_000;
/// The Java soak's takeover of 1,000 shards, segment_blob with log commits,
/// after the shared-lease-read change: 57.0 s.
const JAVA_MILLIS: u64 = 57_000;

/// Takes the dead node's `SHARDS` shards with `max_concurrent` takes at once;
/// answers the pass's wall time.
fn takeover_time(max_concurrent: usize) -> Duration {
    takeover_within(max_concurrent, Duration::from_secs(3_600)).0
}

/// As [`takeover_time`], the pass allowed `pass` to start takes: its time and
/// how many shards it took.
fn takeover_within(max_concurrent: usize, pass_for: Duration) -> (Duration, usize, usize) {
    let c = cluster_of(4 << 30);
    create_n(&c, "logs", "u1", SHARDS);
    let one = node(&c, "node-1", 1, false, None);
    // Three documents a shard on average: nearly every shard gets one.
    for n in 0..DOCS {
        write_id(&one.role, &format!("d{n}"));
    }
    let named = one.role.held_shards();
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    // Mean 23.5 ms a request: the soak's 20 ms with 1 request in 50 at 200 ms.
    for op in [S3Op::Get, S3Op::GetRange, S3Op::Head, S3Op::Put, S3Op::List] {
        c.store.set_latency(
            op,
            Latency {
                base: Duration::from_millis(20),
                jitter: Duration::from_millis(7),
            },
        );
    }
    let mut cfg = config(true);
    cfg.max_concurrent = max_concurrent;
    cfg.pass = pass_for;
    let pass = ProactiveTakeover::new("node-2", cfg, Arc::clone(&two.role), predecessors);
    let total = |c: &Cluster| c.store.meter().total();
    let lists = c.store.meter().count(S3Op::List);
    let requests = total(&c);
    let start = Instant::now();
    let report = pass.pass(&dead(&one));
    let took = start.elapsed();
    assert_eq!(report.taken, named.len(), "{report:?}");
    assert_eq!(c.store.meter().count(S3Op::List), lists, "no LIST");
    eprintln!(
        "takeover of {} shards, {max_concurrent} at once: {took:?}, {:.1} requests a shard",
        named.len(),
        (total(&c) - requests) as f64 / named.len() as f64
    );
    for op in [S3Op::Get, S3Op::GetRange, S3Op::Head, S3Op::Put, S3Op::List] {
        c.store.set_latency(op, Latency::default());
    }
    for shard in &named {
        assert_eq!(
            two.role.max_seq_no(&shard.uuid, shard.shard),
            one.role.max_seq_no(&shard.uuid, shard.shard),
            "shard {} lost documents",
            shard.shard
        );
    }
    (took, report.taken, named.len())
}

// docs/specs/takeover-scale AC-1, AC-2, AC-3
#[test]
#[ignore = "evidence: run by hand with --nocapture"]
fn a_dead_nodes_thousand_shards_are_taken_within_a_tenth_of_the_java_time() {
    let shipped = takeover_time(4);
    let wide = takeover_time(16);
    eprintln!("Java {JAVA_MILLIS} ms; shipped {shipped:?}; 16 at once {wide:?}");
    assert!(
        wide.as_millis() <= u128::from(JAVA_MILLIS) * 11 / 10,
        "16 at once took {wide:?} against the Java's {JAVA_MILLIS} ms"
    );
}

/// Eight shards of a dead node held from eight threads take about as long as
/// one: the opens of different shards do not wait for each other.
// docs/specs/takeover-scale AC-4
#[test]
fn opening_different_shards_at_once_takes_the_time_of_opening_one() {
    let c = cluster_of(1 << 30);
    create_n(&c, "logs", "u1", 8);
    let one = node(&c, "node-1", 1, false, None);
    for ids in ids_by_shard(8, 1) {
        write_id(&one.role, &ids[0]);
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let two = node(&c, "node-2", 2, true, Some(takeover(&c, &one, &calls)));
    for op in [S3Op::Get, S3Op::GetRange, S3Op::Head, S3Op::Put, S3Op::List] {
        c.store
            .set_latency(op, Latency::fixed(Duration::from_millis(20)));
    }
    let info = info_n(8);
    let start = Instant::now();
    two.role.hold(&info, 0).expect("holds");
    let single = start.elapsed();
    let start = Instant::now();
    std::thread::scope(|s| {
        for shard in 1..8 {
            let (role, info) = (&two.role, &info);
            s.spawn(move || role.hold(info, shard).expect("holds"));
        }
    });
    let seven = start.elapsed();
    assert!(
        seven < single * 3,
        "seven shards at once took {seven:?}, one took {single:?}"
    );
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[(((sorted.len() - 1) as f64) * p) as usize]
}

/// Writes by the node that runs a pass, to a shard of its own, while the pass
/// takes a dead node's 64 shards `max_concurrent` at a time (none: no pass),
/// at S3-like latency.
fn write_latencies(max_concurrent: Option<usize>) -> Vec<Duration> {
    let c = cluster_of(1 << 30);
    create_n(&c, "logs", "u1", 64);
    create_n(&c, "other", "u9", 1);
    let one = node(&c, "node-1", 1, false, None);
    for n in 0..192 {
        write_id(&one.role, &format!("d{n}"));
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    // The writes go through the node that runs the pass, to a shard of its own.
    write_in(&two.role, "other", "warm");
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
    let mut cfg = config(true);
    // No pass runs when `max_concurrent` is None; the width is then unused.
    cfg.max_concurrent = max_concurrent.unwrap_or(1);
    let pass = ProactiveTakeover::new("node-2", cfg, Arc::clone(&two.role), predecessors);
    let lease = dead(&one);
    std::thread::scope(|s| {
        if max_concurrent.is_some() {
            s.spawn(|| pass.pass(&lease));
        }
        (0..200)
            .map(|n| {
                let start = Instant::now();
                write_in(&two.role, "other", &format!("w{n}"));
                start.elapsed()
            })
            .collect()
    })
}

// docs/specs/takeover-concurrency-default AC-3
#[test]
#[ignore = "evidence for the default of max_concurrent: run by hand with --nocapture"]
fn write_latency_of_a_live_node_while_a_pass_runs_wide() {
    let mut p99 = Vec::new();
    for (label, width) in [
        ("no pass", None),
        ("4 wide", Some(4)),
        ("16 wide", Some(16)),
    ] {
        let mut all: Vec<Duration> = (0..3).flat_map(|_| write_latencies(width)).collect();
        all.sort();
        p99.push(percentile(&all, 0.99));
        eprintln!(
            "{label}: p50 {:?} p99 {:?} ({} writes)",
            percentile(&all, 0.5),
            p99.last().unwrap(),
            all.len()
        );
    }
    let ratio = p99[2].as_secs_f64() / p99[0].as_secs_f64();
    eprintln!("p99 16 wide over no pass {ratio:.3}");
    assert!(ratio <= 1.05, "write p99 {ratio:.3}x with a 16-wide pass");
}

// docs/specs/takeover-concurrency-default AC-2: 16 at once for 60 s.
#[test]
#[ignore = "evidence: run by hand with --nocapture"]
fn one_pass_at_the_shipped_settings_takes_every_shard() {
    let (took, taken, named) = takeover_within(16, Duration::from_secs(60));
    eprintln!("one 60 s pass at 16 wide took {took:?}: {taken} of {named} shards");
    assert_eq!(taken, named);
    assert!(took.as_millis() <= u128::from(JAVA_MILLIS) * 11 / 10);
}
