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
use oss_node::descriptor_routes::DescriptorRoutes;
use oss_node::writer::Routes;
use oss_node::writer::{RoleTarget, ShardCap, Takeover};
use oss_writer::bulk::ItemResult;
use oss_writer::bulk::{IndexInfo, Owner, Planned, Target};
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
    full_with(max, held, hand_off, |routes| Arc::new(routes))
}

/// [`full`] over routes `wrap` makes.
fn full_with(
    max: usize,
    held: usize,
    hand_off: bool,
    wrap: impl FnOnce(DescriptorRoutes) -> Arc<dyn Routes>,
) -> (Cluster, Node) {
    let c = cluster();
    let names = [("a", "ua"), ("b", "ub"), ("c", "uc"), ("d", "ud")];
    for (name, uuid) in names {
        create(&c, name, uuid);
    }
    let store: Arc<dyn BlobStore> = c.store.clone();
    let one = node_routed(
        &c,
        "node-1",
        1,
        false,
        Some(Takeover::from_leases(store, "", "node-1", 1, 1 << 20)),
        wrap,
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

fn owner_of(one: &Node, name: &str) -> Owner {
    let mut info = info();
    info.name = name.into();
    info.uuid = format!("u{name}");
    RoleTarget::new(&one.role).owner(&info, 0)
}

#[test]
fn a_write_for_an_unheld_shard_goes_to_the_member_with_room_when_this_node_is_nearly_full() {
    let (_c, one) = full(4, 4, true);
    pass(&one, &[peer("node-2", 1, 10)]);
    // The pass let one go; write it back so the node is nearly full again.
    assert!(matches!(
        write_in(&one.role, "a", "2"),
        ItemResult::Written { .. }
    ));
    assert_eq!(one.role.shard_cap_stats().held, 4);
    assert_eq!(owner_of(&one, "e"), Owner::Node("node-2".into()));
    assert_eq!(one.role.shard_cap_stats().steered, 1);
}

#[test]
fn a_held_shard_is_never_steered_away() {
    let (_c, one) = full(4, 4, true);
    pass(&one, &[peer("node-2", 1, 10)]);
    assert!(matches!(
        write_in(&one.role, "a", "2"),
        ItemResult::Written { .. }
    ));
    assert_eq!(owner_of(&one, "b"), Owner::Local);
    assert_eq!(one.role.shard_cap_stats().steered, 0);
}

#[test]
fn a_write_is_not_steered_with_the_setting_off_or_a_node_with_room_or_no_member_with_room() {
    let (_c, off) = full(4, 4, false);
    off.role
        .hand_off(&[peer("node-2", 1, 10)], 0, TTL, "node-1");
    assert_ne!(owner_of(&off, "e"), Owner::Node("node-2".into()));

    let (_c, roomy) = full(4, 2, true);
    roomy
        .role
        .hand_off(&[peer("node-2", 1, 10)], 0, TTL, "node-1");
    assert_ne!(owner_of(&roomy, "e"), Owner::Node("node-2".into()));

    let (_c, packed) = full(4, 4, true);
    packed
        .role
        .hand_off(&[peer("node-2", 9, 10)], 0, TTL, "node-1");
    assert_ne!(owner_of(&packed, "e"), Owner::Node("node-2".into()));
    assert_eq!(packed.role.shard_cap_stats().steered, 0);
}

#[test]
fn a_steered_shard_keeps_going_to_the_same_member_so_its_refresh_follows_its_write() {
    let (_c, one) = full(4, 4, true);
    pass(&one, &[peer("node-2", 1, 10)]);
    assert!(matches!(
        write_in(&one.role, "a", "2"),
        ItemResult::Written { .. }
    ));
    assert_eq!(owner_of(&one, "e"), Owner::Node("node-2".into()));
    // The fleet changes: no member has room now, and node-3 would be picked.
    one.role.hand_off(
        &[peer("node-2", 9, 10), peer("node-3", 1, 10)],
        0,
        TTL,
        "node-1",
    );
    assert_eq!(owner_of(&one, "e"), Owner::Node("node-2".into()));
    assert_eq!(one.role.shard_cap_stats().steered, 1, "counted once");
}

/// Routes that name `node-3` the owner of index `e` and defer to the
/// descriptors for the rest.
struct ThreeOwnsE(DescriptorRoutes);

impl Routes for ThreeOwnsE {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        self.0.index(name)
    }

    fn owner(&self, index: &IndexInfo, shard: u32) -> Owner {
        if index.name == "e" {
            Owner::Node("node-3".into())
        } else {
            self.0.owner(index, shard)
        }
    }

    fn forward(
        &self,
        node: &str,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<ItemResult> {
        self.0.forward(node, index, shard, group)
    }
}

// docs/specs/writer-steering AC-5
#[test]
fn a_shard_the_routes_give_another_node_is_not_steered() {
    let (_c, one) = full_with(4, 4, true, |routes| Arc::new(ThreeOwnsE(routes)));
    pass(&one, &[peer("node-2", 1, 10)]);
    assert!(matches!(
        write_in(&one.role, "a", "2"),
        ItemResult::Written { .. }
    ));
    // Nearly full with a member with room, yet the routes' owner stands.
    assert_eq!(owner_of(&one, "e"), Owner::Node("node-3".into()));
    assert_eq!(one.role.shard_cap_stats().steered, 0);
}
