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
    cfg.pass = Duration::from_secs(3_600);
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
    took
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
