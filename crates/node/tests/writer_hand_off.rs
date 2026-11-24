//! docs/specs/writer-hand-off: a writer node nearly at its cap, or far above
//! the fleet's mean, hands idle shards to a member with room.

// ARITH: test arithmetic on counts; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, S3Op};
use oss_cluster::heads::shard_head_key;
use oss_format::{NodeLease, ShardHead};
use oss_node::writer::{ShardCap, Takeover};
use oss_writer::bulk::ItemResult;
use support::*;

const TTL: i64 = 30_000;

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

fn pause() {
    std::thread::sleep(Duration::from_millis(30));
}

/// node-1 capped at `max`, over indexes `a` to `d`, each written once with a
/// pause between so the first written is the idlest.
fn full(max: usize, held: usize, hand_off: bool) -> (Cluster, Node) {
    let c = cluster();
    let names = [("a", "ua"), ("b", "ub"), ("c", "uc"), ("d", "ud")];
    for (name, uuid) in names {
        create(&c, name, uuid);
    }
    let store: Arc<dyn BlobStore> = c.store.clone();
    let one = node(
        &c,
        "node-1",
        1,
        false,
        Some(Takeover::from_leases(store, "", "node-1", 1, 1 << 20)),
    );
    assert!(one.role.set_shard_cap(ShardCap {
        max,
        evict_after_millis: 1,
    }));
    one.role.set_hand_off(hand_off);
    for (name, _) in names.iter().take(held) {
        assert!(matches!(
            write_in(&one.role, name, "1"),
            ItemResult::Written { .. }
        ));
        pause();
    }
    (c, one)
}

/// A live ingest member holding `held` of `cap`.
fn peer(id: &str, held: i32, cap: i32) -> NodeLease {
    let mut lease = NodeLease::new(id, "e", "addr", i64::MAX);
    lease.set_roles(["ingest".to_owned()]);
    lease.held = held;
    lease.cap = cap;
    lease
}

fn pass(one: &Node, members: &[NodeLease]) -> usize {
    one.role.hand_off(members, 0, TTL, "node-1")
}

fn head_owner(c: &Cluster, index: &str) -> Option<String> {
    let mut buf = vec![0u8; 1 << 16];
    let read = c
        .store
        .read_register(&shard_head_key("", index, 0), &mut buf)
        .expect("reads")
        .expect("a head");
    ShardHead::parse(&buf[..read.len])
        .expect("parses")
        .owner_node_id
}

#[test]
fn a_node_nearly_full_hands_off_one_idle_shard_to_a_member_with_room() {
    let (_c, one) = full(4, 4, true);
    assert_eq!(one.role.shard_cap_stats().held, 4);
    assert_eq!(pass(&one, &[peer("node-2", 1, 10)]), 1);
    assert_eq!(one.role.shard_cap_stats().held, 3);
    assert!(!one.role.held("ua", 0), "the idlest went");
    assert_eq!(one.role.shard_cap_stats().handed_off, 1);
}

#[test]
fn a_node_hands_off_nothing_when_every_member_is_full_too() {
    let (_c, one) = full(4, 4, true);
    assert_eq!(pass(&one, &[peer("node-2", 9, 10)]), 0);
    assert_eq!(one.role.shard_cap_stats().held, 4);
}

#[test]
fn a_node_with_the_setting_off_hands_off_nothing() {
    let (_c, one) = full(4, 4, false);
    assert_eq!(pass(&one, &[peer("node-2", 1, 10)]), 0);
    assert_eq!(one.role.shard_cap_stats().held, 4);
}

#[test]
fn a_node_under_its_share_hands_off_nothing_and_asks_the_store_nothing() {
    let (c, one) = full(4, 2, true);
    let before = requests(&c);
    assert_eq!(pass(&one, &[peer("node-2", 1, 10)]), 0);
    assert_eq!(requests(&c), before);
}

#[test]
fn a_node_far_above_the_mean_hands_off_towards_it() {
    // Holds 3 of 4; the mean over it and an empty member is 1.5, and 3 is over 1.5 * 1.5.
    let (_c, one) = full(4, 3, true);
    assert_eq!(pass(&one, &[peer("node-2", 0, 10)]), 1);
    assert_eq!(one.role.shard_cap_stats().held, 2);
    // Down to 1.25 * 1.5 rounded up is 2: no further pass lets go.
    assert_eq!(pass(&one, &[peer("node-2", 0, 10)]), 0);
}

#[test]
fn a_handed_off_shard_gives_its_head_back_and_reopens_with_its_documents() {
    let (c, one) = full(4, 4, true);
    assert_eq!(pass(&one, &[peer("node-2", 1, 10)]), 1);
    assert_eq!(head_owner(&c, "a"), None);
    pause();
    match write_in(&one.role, "a", "1") {
        ItemResult::Written { written, .. } => {
            assert_eq!(written.outcome, oss_writer::index::Outcome::Updated);
        }
        other => panic!("{other:?}"),
    }
}
