//! docs/specs/takeover-budgets: a node takes a shard its writer released.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oss_blob::BlobStore;
use oss_node::writer::Takeover;
use oss_testkit::budget;
use support::*;

const DELAY: Duration = Duration::from_millis(20);

fn now() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("after 1970")
            .as_millis(),
    )
    .expect("fits")
}

fn takeover_of(c: &Cluster, id: &str, term: i64) -> Takeover {
    let store: Arc<dyn BlobStore> = c.io();
    Takeover::from_leases(store, "", id, term, 1 << 20)
}

// docs/specs/takeover-budgets AC-1, AC-2, AC-3
#[test]
fn taking_a_shard_its_writer_released_costs_what_the_table_says() {
    let c = cluster_rounds();
    let rounds = c.rounds.clone().expect("a round counter");
    create(&c, "logs", "u1");
    let one = node(&c, "node-1", 1, false, Some(takeover_of(&c, "node-1", 1)));
    write(&one.role, "a");
    let report = one.role.retire(now(), u64::MAX);
    assert_eq!((report.hibernated, report.given_back), (1, 1), "{report:?}");

    let two = node(&c, "node-2", 2, false, Some(takeover_of(&c, "node-2", 2)));
    // The node's own lease enrolment is its first request, not the take's.
    create(&c, "warm", "u2");
    write_in(&two.role, "warm", "w");
    rounds.reset(DELAY);
    let before = rounds.meter().total();
    write(&two.role, "b");
    let detail = rounds.detail();
    let requests = rounds.meter().total() - before;
    let depth = u64::try_from(rounds.depth()).expect("fits");
    rounds.reset(Duration::ZERO);
    assert_eq!(rounds.meter().count(oss_blob::S3Op::List), 0, "{detail}");
    budget::check("take.released.requests", requests, &detail);
    budget::check("take.released.round_trips", depth, &detail);
    assert!(two.role.held("u1", 0));
    assert_eq!(seq(&write(&two.role, "c")), 2, "a and b came before");
}
