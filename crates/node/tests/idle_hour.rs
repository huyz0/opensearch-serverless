//! docs/specs/idle-budgets: a writer holding or sleeping shards with nothing
//! written makes no request for an hour of its periodic passes.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

mod proactive_support;

use std::sync::Arc;

use oss_blob::{BlobStore, S3Op};
use oss_node::writer::WriterRole;
use oss_testkit::budget;
use proactive_support::*;

const SHARDS: u32 = 4;
/// The publish and hibernate passes run every 250 ms, the janitor every 60 s.
const PUBLISH_PASSES: u32 = 3_600 * 4;
const JANITOR_PASSES: u32 = 60;

fn requests(c: &Cluster) -> u64 {
    [
        S3Op::Get,
        S3Op::GetRange,
        S3Op::Put,
        S3Op::List,
        S3Op::Delete,
    ]
    .into_iter()
    .map(|op| c.store.meter().count(op))
    .sum()
}

/// A node holding four published shards, swept once.
fn held() -> (Cluster, Node) {
    let c = cluster();
    create_n(&c, "logs", "u1", SHARDS);
    let one = node(&c, "node-1", 1, false, None);
    for ids in ids_by_shard(SHARDS, 1) {
        write_id(&one.role, &ids[0]);
    }
    for shard in 0..SHARDS {
        one.role.refresh(&info_n(SHARDS), shard).expect("published");
    }
    (c, one)
}

/// One hour of the node's periodic passes over its shards.
fn an_hour(role: &Arc<WriterRole>, pass: &oss_node::writer::JanitorPass, now: u64) {
    let mut janitor = 0;
    for tick in 0..PUBLISH_PASSES {
        role.publish_due();
        role.hibernate_due(now + u64::from(tick) * 250);
        if tick % (PUBLISH_PASSES / JANITOR_PASSES) == 0 {
            pass.run(gc_config(false));
            janitor += 1;
        }
    }
    assert_eq!(janitor, JANITOR_PASSES);
}

// docs/specs/idle-budgets AC-1, AC-3
#[test]
fn an_hour_of_passes_over_idle_held_shards_asks_for_nothing() {
    let (c, one) = held();
    let pass = janitor(&c, &one.role);
    // The sweep that follows the publishes is work; the hour after is idle.
    pass.run(gc_config(false));
    pass.run(gc_config(false));
    let before = requests(&c);
    an_hour(&one.role, &pass, 1_000);
    budget::check(
        "held_idle.index_requests_target",
        requests(&c) - before,
        "an hour of the publish, hibernate and janitor passes over four idle held shards",
    );
    assert_eq!(one.role.held_shards().len(), SHARDS as usize);
}

// docs/specs/idle-budgets AC-2, AC-3
#[test]
fn an_hour_of_passes_over_hibernated_shards_asks_for_nothing() {
    let (c, one) = held();
    let pass = janitor(&c, &one.role);
    assert_eq!(
        one.role.hibernate_idle(0, 1_000).hibernated,
        SHARDS as usize
    );
    let before = requests(&c);
    an_hour(&one.role, &pass, 2_000);
    budget::check(
        "hibernated_idle.index_requests_target",
        requests(&c) - before,
        "an hour of the publish, hibernate and janitor passes over four hibernated shards",
    );
    assert_eq!(one.role.hibernated(), SHARDS as usize);
}
