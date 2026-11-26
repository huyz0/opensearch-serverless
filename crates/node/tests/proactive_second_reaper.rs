//! docs/specs/proactive-takeover AC-5: the second reaper of a dead node.

// ARITH: test arithmetic on request counts; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use oss_writer::takeover::Predecessors;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use oss_blob::{BlobStore as _, S3Op};
use oss_node::writer::{ProactiveConfig, ProactiveTakeover};
use support::*;

/// node-1 writes to every one of `shards` shards and dies, none published.
fn dead_writer(c: &Cluster, shards: u32) -> Node {
    create_n(c, "logs", "u1", shards);
    let one = node(c, "node-1", 1, false, None);
    for ids in ids_by_shard(shards, 1) {
        write_id(&one.role, &ids[0]);
    }
    one
}

/// A pass by `id` over `one`'s log with its own reader of it.
fn reaper(
    c: &Cluster,
    one: &Node,
    id: &str,
    term: u64,
    cfg: ProactiveConfig,
) -> (Node, ProactiveTakeover, Arc<Predecessors>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover_as(c, one, id, i64::try_from(term).expect("term"), &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let predecessors2 = Arc::clone(&predecessors);
    let me = node(c, id, term, true, Some(take));
    let pass = ProactiveTakeover::new(id, cfg, Arc::clone(&me.role), predecessors);
    (me, pass, predecessors2)
}

/// Every request the store has counted so far.
fn requests(c: &Cluster) -> u64 {
    S3Op::ALL.iter().map(|op| c.store.meter().count(*op)).sum()
}

// docs/specs/proactive-takeover AC-5: the second reaper takes only what is still needed, for at
// most twice the first's requests
#[test]
fn a_second_reaper_after_the_first_costs_at_most_twice_as_much_and_takes_nothing() {
    let c = cluster();
    let one = dead_writer(&c, 4);
    let (two, p2, _) = reaper(&c, &one, "node-2", 2, config(true));
    let (three, p3, _) = reaper(&c, &one, "node-3", 3, config(true));
    let lease = dead(&one);
    let before = requests(&c);
    let first = p2.pass(&lease);
    let first_cost = requests(&c) - before;
    assert_eq!((first.named, first.taken), (4, 4), "{first:?}");
    let before = requests(&c);
    let second = p3.pass(&lease);
    let second_cost = requests(&c) - before;
    assert_eq!(
        (second.taken, second.skipped),
        (0, 4),
        "nothing is left to take: {second:?}"
    );
    assert!(
        second_cost <= 2 * first_cost,
        "the second reaper cost {second_cost} requests, the first {first_cost}"
    );
    assert_eq!(two.role.held_shards().len(), 4);
    assert!(three.role.held_shards().is_empty());
}

// docs/specs/proactive-takeover AC-5: the first reaper is down, so the second takes them all
#[test]
fn a_second_reaper_takes_every_shard_when_the_first_never_runs() {
    let c = cluster();
    let one = dead_writer(&c, 4);
    let (_down, _, _) = reaper(&c, &one, "node-2", 2, config(true));
    let (three, p3, _) = reaper(&c, &one, "node-3", 3, config(true));
    let lease = dead(&one);
    let report = p3.pass(&lease);
    assert_eq!(
        (report.named, report.taken, report.deferred),
        (4, 4, 0),
        "{report:?}"
    );
    assert_eq!(three.role.held_shards().len(), 4);
}
