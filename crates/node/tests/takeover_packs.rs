//! docs/specs/pack-takeover-scale: a dead node's 1,000 packed shards taken by
//! the proactive pass, timed against the same takeover over blob-placed
//! commits. Evidence run by hand: `cargo test -p oss-node --features writer
//! --release --test takeover_packs -- --ignored --nocapture`.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

mod proactive_support;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

use oss_blob::S3Op;
use oss_blob::emulator::faults::Latency;
use oss_node::writer::{DescriptorIncarnation, Incarnation, PackConfig, ProactiveTakeover};
use proactive_support::*;

const SHARDS: u32 = 1_000;
const WIDTH: usize = 16;
/// Documents a shard gets four of on average, and half as many again after.
const DOCS: u32 = 4_000;

fn packs() -> PackConfig {
    PackConfig {
        max_blob_bytes: 8 << 20,
        linger: Duration::from_millis(1),
        publish_threads: 8,
        collect: false,
    }
}

fn incarnation(c: &Cluster) -> (Arc<dyn Incarnation>, Duration) {
    (
        Arc::new(DescriptorIncarnation::new(Arc::clone(&c.descriptors))),
        Duration::from_secs(600),
    )
}

/// What a takeover cost: its wall time and the requests it made.
struct Taken {
    took: Duration,
    requests: u64,
}

/// One dead node's `SHARDS` shards, each with a published commit and one more
/// document held only by the log, taken by a second node; both nodes pack
/// when `packed`.
fn takeover_of_published_shards(packed: bool) -> Taken {
    let c = cluster_of(4 << 30);
    create_n(&c, "logs", "u1", SHARDS);
    let config_packs = packed.then(packs);
    let one = node_with_packs(
        &c,
        "node-1",
        1,
        false,
        None,
        |routes| Arc::new(routes),
        incarnation(&c),
        config_packs,
    );
    let info = info_n(SHARDS);
    // Documents that nearly every shard gets, published; then more that stay
    // in the log.
    for n in 0..DOCS {
        write_id(&one.role, &format!("p{n}"));
    }
    for shard in one.role.held_shards() {
        one.role.refresh(&info, shard.shard).expect("published");
    }
    for n in 0..DOCS / 2 {
        write_id(&one.role, &format!("l{n}"));
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node_with_packs(
        &c,
        "node-2",
        2,
        true,
        Some(take),
        |routes| Arc::new(routes),
        incarnation(&c),
        config_packs,
    );
    for op in [S3Op::Get, S3Op::GetRange, S3Op::Head, S3Op::Put, S3Op::List] {
        c.store.set_latency(
            op,
            Latency {
                base: Duration::from_millis(20),
                jitter: Duration::from_millis(7),
            },
        );
    }
    let base = c.base();
    let mut cfg = config(true);
    cfg.max_concurrent = WIDTH;
    cfg.pass = Duration::from_secs(3_600);
    let pass = ProactiveTakeover::new("node-2", cfg, Arc::clone(&two.role), predecessors);
    let lists = base.meter().count(S3Op::List);
    let requests = base.meter().total();
    let start = Instant::now();
    let report = pass.pass(&dead(&one));
    let took = start.elapsed();
    // Only a shard whose log still holds a write is named: a published one
    // has nothing to replay.
    let named = two.role.held_shards();
    assert_eq!(report.taken, report.named, "{report:?}");
    assert_eq!(named.len(), report.taken, "{report:?}");
    assert!(report.taken >= 800, "{report:?}");
    assert_eq!(base.meter().count(S3Op::List), lists, "no LIST");
    let spent = base.meter().total() - requests;
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
    eprintln!(
        "{} takeover of {} shards: {took:?}, {spent} requests",
        if packed { "packed" } else { "blob" },
        named.len()
    );
    Taken {
        took,
        requests: spent,
    }
}

// docs/specs/pack-takeover-scale AC-1, AC-2, AC-3
#[test]
#[ignore = "evidence: run by hand with --release --nocapture"]
fn a_dead_nodes_packed_shards_are_taken_within_a_tenth_of_its_blob_shards() {
    let blob = takeover_of_published_shards(false);
    let packed = takeover_of_published_shards(true);
    eprintln!(
        "blob {:?}, {} requests; packed {:?}, {} requests",
        blob.took, blob.requests, packed.took, packed.requests
    );
    assert!(
        packed.took.as_millis() * 10 <= blob.took.as_millis() * 11,
        "packed took {:?} against blob {:?}",
        packed.took,
        blob.took
    );
    assert!(
        packed.requests * 10 <= blob.requests * 11,
        "packed made {} requests against blob {}",
        packed.requests,
        blob.requests
    );
}
