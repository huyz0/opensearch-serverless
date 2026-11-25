//! docs/specs/writer-shard-cap: a writer node at its shard cap lets an idle
//! shard go to open the next, and refuses when every shard is in use.

// ARITH: test arithmetic on counts; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, S3Op};
use oss_cluster::heads::shard_head_key;
use oss_format::ShardHead;
use oss_node::writer::{ShardCap, Takeover};
use oss_writer::bulk::IndexInfo;
use oss_writer::bulk::ItemResult;
use support::*;

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

fn index_a() -> IndexInfo {
    IndexInfo {
        name: "a".into(),
        uuid: "ua".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
        default_pipeline: None,
        final_pipeline: None,
    }
}

/// node-1 over three one-shard indexes `a`, `b` and `c`, capped at `max`.
fn capped(max: usize, evict_after_millis: u64) -> (Cluster, Node) {
    let c = cluster();
    for (name, uuid) in [("a", "ua"), ("b", "ub"), ("c", "uc")] {
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
        evict_after_millis,
    }));
    (c, one)
}

fn landed(result: &ItemResult) -> bool {
    matches!(result, ItemResult::Written { .. })
}

fn head(c: &Cluster, index: &str) -> ShardHead {
    let mut buf = vec![0u8; 1 << 16];
    let read = c
        .store
        .read_register(&shard_head_key("", index, 0), &mut buf)
        .expect("reads")
        .expect("a head");
    ShardHead::parse(&buf[..read.len]).expect("parses")
}

fn pause() {
    std::thread::sleep(Duration::from_millis(30));
}

#[test]
fn no_cap_opens_every_shard_and_evicts_none() {
    let (c, one) = capped(0, 1);
    for (name, _) in [("a", 0), ("b", 0), ("c", 0)] {
        assert!(landed(&write_in(&one.role, name, "1")));
    }
    pause();
    for uuid in ["ua", "ub", "uc"] {
        assert!(one.role.held(uuid, 0), "{uuid}");
    }
    let stats = one.role.shard_cap_stats();
    assert_eq!((stats.refused, stats.evicted_for_room), (0, 0));
    drop(c);
}

#[test]
fn at_the_cap_an_idle_shard_goes_to_open_the_next() {
    let (_c, one) = capped(2, 1);
    assert!(landed(&write_in(&one.role, "a", "1")));
    pause();
    assert!(landed(&write_in(&one.role, "b", "1")));
    pause();
    assert!(landed(&write_in(&one.role, "c", "1")));
    assert!(one.role.held("uc", 0));
    assert!(!one.role.held("ua", 0), "the shard unused longest went");
    assert!(one.role.held("ub", 0));
    let stats = one.role.shard_cap_stats();
    assert_eq!(stats.held, 2);
    assert_eq!(stats.max, 2);
    assert_eq!(stats.evicted_for_room, 1);
    assert_eq!(stats.refused, 0);
}

// docs/specs/takeover-scale AC-4: opens at once still keep the cap.
#[test]
fn opens_at_once_under_a_cap_open_the_cap_and_refuse_the_rest() {
    let (_c, one) = capped(2, 60_000);
    let landed_count = std::thread::scope(|s| {
        let handles: Vec<_> = ["a", "b", "c"]
            .into_iter()
            .map(|index| {
                let role = &one.role;
                s.spawn(move || landed(&write_in(role, index, "1")))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("joins"))
            .filter(|landed| *landed)
            .count()
    });
    assert_eq!(landed_count, 2);
    assert_eq!(one.role.held_shards().len(), 2);
    assert_eq!(one.role.shard_cap_stats().refused, 1);
}

#[test]
fn at_the_cap_with_every_shard_in_use_the_open_is_refused_and_counted() {
    let (c, one) = capped(2, 60_000);
    assert!(landed(&write_in(&one.role, "a", "1")));
    assert!(landed(&write_in(&one.role, "b", "1")));
    let (before, puts) = (requests(&c), c.store.meter().count(S3Op::Put));
    match write_in(&one.role, "c", "1") {
        ItemResult::Failed { status, error } => {
            assert_eq!(status, 503);
            assert!(format!("{error:?}").contains("shard cap"), "{error:?}");
        }
        other => panic!("{other:?}"),
    }
    // The descriptor read that names the index is the write path's, not the cap's.
    assert!(
        requests(&c) - before <= 1,
        "a refusal reads at most the descriptor"
    );
    assert_eq!(c.store.meter().count(S3Op::Put), puts, "and writes nothing");
    assert!(one.role.held("ua", 0) && one.role.held("ub", 0));
    assert!(!one.role.held("uc", 0));
    let stats = one.role.shard_cap_stats();
    assert_eq!((stats.refused, stats.evicted_for_room), (1, 0));
}

#[test]
fn eviction_off_means_the_cap_only_refuses() {
    let (_c, one) = capped(1, 0);
    assert!(landed(&write_in(&one.role, "a", "1")));
    pause();
    assert!(!landed(&write_in(&one.role, "b", "1")));
    assert!(one.role.held("ua", 0));
    let stats = one.role.shard_cap_stats();
    assert_eq!((stats.refused, stats.evicted_for_room), (1, 0));
}

#[test]
fn an_evicted_shard_keeps_its_writes_and_is_taken_again_on_its_next_write() {
    let (c, one) = capped(1, 1);
    assert!(landed(&write_in(&one.role, "a", "1")));
    pause();
    assert!(landed(&write_in(&one.role, "b", "1")));
    assert!(!one.role.held("ua", 0));
    // The head was given back, not left naming this node.
    assert_eq!(head(&c, "a").owner_node_id, None);
    pause();
    // `1` is there, so this is an update: the earlier write survived.
    match write_in(&one.role, "a", "1") {
        ItemResult::Written { written, .. } => {
            assert_eq!(written.outcome, oss_writer::index::Outcome::Updated);
        }
        other => panic!("{other:?}"),
    }
    assert!(one.role.held("ua", 0));
    assert!(!one.role.held("ub", 0), "b went to make room for a");
    assert_eq!(head(&c, "a").owner_node_id.as_deref(), Some("node-1"));
}

#[test]
fn a_head_that_cannot_be_given_back_leaves_the_shard_asleep_at_its_term() {
    let (c, one) = capped(1, 1);
    assert!(landed(&write_in(&one.role, "a", "1")));
    let index = index_a();
    // Published, so the eviction's only write is the head's release.
    one.role.refresh(&index, 0).expect("published");
    pause();
    let term = head(&c, "a").term;
    c.store.fail_next(S3Op::Put, 1);
    assert!(landed(&write_in(&one.role, "b", "1")));
    assert!(!one.role.held("ua", 0));
    assert!(one.role.asleep("ua", 0));
    assert_eq!(head(&c, "a").owner_node_id.as_deref(), Some("node-1"));
    assert_eq!(head(&c, "a").term, term);
}

#[test]
fn evicting_a_published_shard_costs_one_put_and_a_shard_with_unpublished_writes_one_more() {
    let (c, one) = capped(1, 1);
    assert!(landed(&write_in(&one.role, "a", "1")));
    one.role.refresh(&index_a(), 0).expect("published");
    pause();
    let (all, puts) = (requests(&c), c.store.meter().count(S3Op::Put));
    assert_eq!(one.role.make_room(u64::MAX), 1);
    assert_eq!(requests(&c) - all, 2, "the head read and its release");
    assert_eq!(c.store.meter().count(S3Op::Put) - puts, 1);

    // Unpublished writes: the publish is paid first.
    assert!(landed(&write_in(&one.role, "b", "1")));
    pause();
    let puts = c.store.meter().count(S3Op::Put);
    assert_eq!(one.role.make_room(u64::MAX), 1);
    assert!(c.store.meter().count(S3Op::Put) - puts > 1);
}

/// node-1 capped at 4 over indexes `a` to `e`, with `a` to `d` written once
/// each, a pause between so `a` is idlest.
fn four_held(percent: u64) -> (Cluster, Node) {
    let c = cluster();
    for (name, uuid) in [
        ("a", "ua"),
        ("b", "ub"),
        ("c", "uc"),
        ("d", "ud"),
        ("e", "ue"),
    ] {
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
        max: 4,
        evict_after_millis: 1,
    }));
    one.role.set_evict_headroom_percent(percent);
    for name in ["a", "b", "c", "d"] {
        assert!(landed(&write_in(&one.role, name, "1")));
        pause();
    }
    (c, one)
}

#[test]
fn headroom_lets_go_of_more_than_the_one_shard_a_burst_needs() {
    let (_c, one) = four_held(50);
    assert!(landed(&write_in(&one.role, "e", "1")));
    // Two let go, one opened: a, b gone; c, d, e held.
    assert_eq!(one.role.shard_cap_stats().evicted_for_room, 2);
    assert_eq!(one.role.shard_cap_stats().held, 3);
}

#[test]
fn with_no_headroom_one_shard_goes_per_open() {
    let (_c, one) = four_held(0);
    assert!(landed(&write_in(&one.role, "e", "1")));
    assert_eq!(one.role.shard_cap_stats().evicted_for_room, 1);
    assert_eq!(one.role.shard_cap_stats().held, 4);
}

#[test]
fn headroom_stops_at_the_shards_that_can_go() {
    let (_c, one) = four_held(100);
    one.role
        .set_pinned_indexes(["b", "c", "d"].map(str::to_owned));
    assert_eq!(one.role.make_room(u64::MAX / 2), 1, "only a can go");
    assert_eq!(one.role.shard_cap_stats().held, 3);
}

#[test]
fn a_pinned_shard_is_not_let_go_of_even_when_idlest() {
    let (_c, one) = four_held(0);
    one.role.set_pinned_indexes(["a".to_owned()]);
    assert!(landed(&write_in(&one.role, "e", "1")));
    assert!(one.role.held("ua", 0), "the idlest is pinned and stays");
    assert!(!one.role.held("ub", 0), "the next idlest went");
}

#[test]
fn a_node_whose_idle_shards_are_all_pinned_refuses_the_open() {
    let (_c, one) = four_held(0);
    one.role
        .set_pinned_indexes(["a", "b", "c", "d"].map(str::to_owned));
    assert!(!landed(&write_in(&one.role, "e", "1")));
    assert_eq!(one.role.shard_cap_stats().refused, 1);
    assert_eq!(one.role.shard_cap_stats().held, 4);
}

#[test]
fn a_pinned_shard_does_not_hibernate() {
    let (_c, one) = four_held(0);
    one.role.set_pinned_indexes(["a".to_owned()]);
    let pass = one.role.hibernate_idle(1, u64::MAX / 2);
    assert!(one.role.held("ua", 0), "pinned stays");
    assert!(!one.role.held("ub", 0), "unpinned sleeps");
    assert_eq!(pass.hibernated, 3);
}
