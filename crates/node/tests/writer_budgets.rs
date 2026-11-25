//! docs/specs/writer-budgets: what a writer node's first holds, its
//! publishes and its release of idle shards cost in requests.

// ARITH: test arithmetic on counts; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

mod proactive_support;

use oss_blob::{BlobStore, S3Op};
use oss_testkit::budget;
use proactive_support::*;

const SHARDS: u32 = 4;

fn total(c: &Cluster) -> u64 {
    c.store.meter().total()
}

fn lists(c: &Cluster) -> u64 {
    c.store.meter().count(S3Op::List)
}

// docs/specs/writer-budgets AC-1
#[test]
fn the_first_take_of_four_never_written_shards_costs_what_the_table_says() {
    let c = cluster();
    create_n(&c, "logs", "u1", SHARDS);
    let one = node(&c, "node-1", 1, false, None);
    let (before, lists_before) = (total(&c), lists(&c));
    let info = info_n(SHARDS);
    for shard in 0..SHARDS {
        one.role.hold(&info, shard).expect("holds");
    }
    assert_eq!(lists(&c), lists_before, "no LIST");
    assert_eq!(one.role.held_shards().len(), SHARDS as usize);
    budget::check(
        "take.fresh.requests",
        total(&c) - before,
        "the first holds of four shards no writer ever wrote",
    );
}

/// A node holding four shards, one document written to each.
fn written() -> (Cluster, Node) {
    let c = cluster();
    create_n(&c, "logs", "u1", SHARDS);
    let one = node(&c, "node-1", 1, false, None);
    let info = info_n(SHARDS);
    for shard in 0..SHARDS {
        one.role.hold(&info, shard).expect("holds");
    }
    for ids in ids_by_shard(SHARDS, 1) {
        write_id(&one.role, &ids[0]);
    }
    (c, one)
}

// docs/specs/writer-budgets AC-2
#[test]
fn a_write_to_each_of_four_held_shards_then_a_publish_costs_what_the_table_says() {
    let c = cluster();
    create_n(&c, "logs", "u1", SHARDS);
    let one = node(&c, "node-1", 1, false, None);
    let info = info_n(SHARDS);
    for shard in 0..SHARDS {
        one.role.hold(&info, shard).expect("holds");
    }
    let (before, lists_before) = (total(&c), lists(&c));
    for ids in ids_by_shard(SHARDS, 1) {
        write_id(&one.role, &ids[0]);
    }
    for shard in 0..SHARDS {
        one.role.refresh(&info, shard).expect("published");
    }

    assert_eq!(lists(&c), lists_before, "no LIST");
    budget::check(
        "publish.requests",
        total(&c) - before,
        "one write to each of four held shards, then a publish of all four",
    );
}

// docs/specs/writer-budgets AC-3
#[test]
fn releasing_four_idle_shards_costs_what_the_table_says() {
    let (c, one) = written();
    for shard in 0..SHARDS {
        one.role.refresh(&info_n(SHARDS), shard).expect("published");
    }
    let (before, lists_before) = (total(&c), lists(&c));
    let report = one.role.retire(1_000, u64::MAX);
    assert_eq!(
        (report.hibernated, report.given_back),
        (SHARDS as usize, SHARDS as usize),
        "{report:?}"
    );
    assert_eq!(lists(&c), lists_before, "no LIST");
    budget::check(
        "release.requests",
        total(&c) - before,
        "retiring a node that holds four published idle shards",
    );
}
