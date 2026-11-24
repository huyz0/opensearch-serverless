//! docs/specs/writer-give-back: a shard asleep for long enough gives its head
//! back, and the next write takes it the ordinary way.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::{BlobStore, S3Op};
use oss_cluster::heads::shard_head_key;
use oss_format::ShardHead;
use oss_node::writer::{GIVE_BACK_PER_PASS, Takeover};
use oss_testkit::budget;
use oss_writer::bulk::ItemResult;
use oss_writer::index::Outcome;
use support::*;

const IDLE: u64 = 1_000;
const GIVE: u64 = 10_000;

fn after(millis: u64) -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("after 1970")
            .as_millis(),
    )
    .expect("fits")
        + millis
}

fn total(c: &Cluster) -> u64 {
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

fn head(c: &Cluster) -> ShardHead {
    let mut buf = vec![0u8; 1 << 16];
    let read = c
        .store
        .read_register(&shard_head_key("", "logs", 0), &mut buf)
        .expect("reads")
        .expect("a head");
    ShardHead::parse(&buf[..read.len]).expect("parses")
}

fn takeover_of(c: &Cluster, id: &str, term: i64) -> Takeover {
    let store: Arc<dyn BlobStore> = c.store.clone();
    Takeover::from_leases(store, "", id, term, 1 << 20)
}

/// node-1 wrote `a` and `b`, published, and slept at `after(2 * IDLE)`.
fn asleep() -> (Cluster, Node, u64) {
    let c = cluster();
    create(&c, "logs", "u1");
    let one = node(&c, "node-1", 1, false, Some(takeover_of(&c, "node-1", 1)));
    assert_eq!(seq(&write(&one.role, "a")), 0);
    assert_eq!(seq(&write(&one.role, "b")), 1);
    let slept = after(2 * IDLE);
    assert_eq!(one.role.hibernate_idle(IDLE, slept).hibernated, 1);
    (c, one, slept)
}

fn outcome(result: &ItemResult) -> Outcome {
    match result {
        ItemResult::Written { written, .. } => written.outcome,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_shard_asleep_long_enough_gives_its_head_back_for_what_the_table_says() {
    let (c, one, slept) = asleep();
    let term = head(&c).term;
    let before = total(&c);
    assert_eq!(one.role.give_back_idle(GIVE, slept + GIVE), 1);
    budget::check(
        "writer.hibernation.give_back.requests",
        total(&c) - before,
        "giving back the head of one hibernated shard",
    );
    let given = head(&c);
    assert_eq!(given.owner_node_id, None);
    assert_eq!(given.term, term, "the term stays");
    assert!(!one.role.asleep("u1", 0), "the record is used up");
    assert_eq!(one.role.hibernated(), 0);
}

#[test]
fn a_shard_asleep_less_is_left_and_a_pass_over_none_due_asks_nothing() {
    let (c, one, slept) = asleep();
    let before = total(&c);
    assert_eq!(one.role.give_back_idle(GIVE, slept + GIVE - 1), 0);
    assert_eq!(total(&c), before);
    assert!(one.role.asleep("u1", 0));
    assert!(head(&c).owner_node_id.is_some());
}

#[test]
fn with_the_threshold_zero_nothing_is_given_back() {
    let (c, one, slept) = asleep();
    assert_eq!(one.role.give_back_due(slept + 100 * GIVE), 0);
    assert!(head(&c).owner_node_id.is_some());
    assert!(one.role.asleep("u1", 0));
}

#[test]
fn a_write_after_a_give_back_takes_the_shard_at_the_next_term_with_every_document() {
    let (c, one, slept) = asleep();
    let term = head(&c).term;
    assert_eq!(one.role.give_back_idle(GIVE, slept + GIVE), 1);
    // `a` is there, so it is an update; `c` is new, and follows a and b.
    assert_eq!(outcome(&write(&one.role, "a")), Outcome::Updated);
    assert_eq!(seq(&write(&one.role, "c")), 3);
    let now = head(&c);
    assert_eq!(now.term, term + 1);
    assert_eq!(now.owner_node_id.as_deref(), Some("node-1"));
}

#[test]
fn another_nodes_write_takes_a_given_back_head_and_this_node_is_then_refused() {
    let (c, one, slept) = asleep();
    assert_eq!(one.role.give_back_idle(GIVE, slept + GIVE), 1);
    // node-2 has no lease to revoke: node-1's is still live, and the head
    // names no owner.
    let two = node(&c, "node-2", 2, false, Some(takeover_of(&c, "node-2", 2)));
    assert_eq!(seq(&write(&two.role, "x")), 2);
    let refused = one.role.hold(&info(), 0).expect_err("node-2 holds it");
    assert_eq!(refused.holder.as_deref(), Some("node-2"), "{refused:?}");
}

#[test]
fn a_write_that_woke_the_shard_first_leaves_nothing_to_give_back() {
    let (c, one, slept) = asleep();
    let term = head(&c).term;
    assert_eq!(outcome(&write(&one.role, "a")), Outcome::Updated);
    assert_eq!(one.role.give_back_idle(GIVE, slept + GIVE), 0);
    let now = head(&c);
    assert_eq!(now.term, term);
    assert_eq!(now.owner_node_id.as_deref(), Some("node-1"));
}

#[test]
fn a_head_that_moved_while_it_slept_is_left_as_the_other_node_wrote_it() {
    let (c, one, slept) = asleep();
    let two = node(&c, "node-2", 2, true, Some(takeover_of(&c, "node-2", 2)));
    assert_eq!(seq(&write(&two.role, "x")), 2);
    let theirs = head(&c);
    assert_eq!(one.role.give_back_idle(GIVE, slept + GIVE), 0);
    assert_eq!(head(&c), theirs);
}

#[test]
fn a_pass_gives_back_no_more_than_its_cap_and_the_rest_are_due_next_pass() {
    let shards = u32::try_from(GIVE_BACK_PER_PASS).expect("fits") + 3;
    let c = cluster();
    create_n(&c, "logs", "u1", shards);
    let one = node(&c, "node-1", 1, false, Some(takeover_of(&c, "node-1", 1)));
    for ids in ids_by_shard(shards, 1) {
        write_id(&one.role, &ids[0]);
    }
    let slept = after(2 * IDLE);
    let report = one.role.hibernate_idle(IDLE, slept);
    assert_eq!(report.hibernated, shards as usize, "{report:?}");
    assert_eq!(
        one.role.give_back_idle(GIVE, slept + GIVE),
        GIVE_BACK_PER_PASS
    );
    assert_eq!(one.role.hibernated(), 3, "the rest stay asleep");
    assert_eq!(one.role.give_back_idle(GIVE, slept + GIVE), 3);
    assert_eq!(one.role.hibernated(), 0);
}
