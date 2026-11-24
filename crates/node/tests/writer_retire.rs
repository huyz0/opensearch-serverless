//! docs/specs/node-retire: a writer role retiring publishes what it holds
//! and gives every head back.

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
use support::*;

fn now() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("after 1970")
            .as_millis(),
    )
    .expect("fits")
}

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

fn owner(c: &Cluster, shard: u32) -> Option<String> {
    let mut buf = vec![0u8; 1 << 16];
    let read = c
        .store
        .read_register(&shard_head_key("", "logs", shard), &mut buf)
        .expect("reads")
        .expect("a head");
    ShardHead::parse(&buf[..read.len])
        .expect("parses")
        .owner_node_id
}

fn takeover_of(c: &Cluster, id: &str, term: i64) -> Takeover {
    let store: Arc<dyn BlobStore> = c.store.clone();
    Takeover::from_leases(store, "", id, term, 1 << 20)
}

// docs/specs/node-retire AC-3
#[test]
fn retiring_publishes_and_gives_back_the_head_and_a_second_retire_asks_nothing() {
    let c = cluster();
    create(&c, "logs", "u1");
    let one = node(&c, "node-1", 1, false, Some(takeover_of(&c, "node-1", 1)));
    assert_eq!(seq(&write(&one.role, "a")), 0);
    assert_eq!(owner(&c, 0).as_deref(), Some("node-1"));
    let before = requests(&c);
    let report = one.role.retire(now(), u64::MAX);
    assert_eq!((report.hibernated, report.given_back), (1, 1), "{report:?}");
    budget::check(
        "writer.retire.one_shard.requests",
        requests(&c) - before,
        "retiring a node that holds one shard with one unpublished write",
    );
    assert!(report.failed.is_empty());
    assert_eq!(owner(&c, 0), None);
    let before = requests(&c);
    let again = one.role.retire(now(), u64::MAX);
    assert_eq!((again.hibernated, again.given_back), (0, 0));
    assert_eq!(requests(&c), before, "nothing held asks nothing");
}

// docs/specs/node-retire AC-4
#[test]
fn retiring_gives_back_more_heads_than_one_pass_may() {
    let shards = u32::try_from(GIVE_BACK_PER_PASS).expect("fits") + 3;
    let c = cluster();
    create_n(&c, "logs", "u1", shards);
    let one = node(&c, "node-1", 1, false, Some(takeover_of(&c, "node-1", 1)));
    for ids in ids_by_shard(shards, 1) {
        write_id(&one.role, &ids[0]);
    }
    let report = one.role.retire(now(), u64::MAX);
    assert_eq!(report.hibernated, shards as usize, "{report:?}");
    assert_eq!(report.given_back, shards as usize, "{report:?}");
    for shard in 0..shards {
        assert_eq!(owner(&c, shard), None, "shard {shard}");
    }
}

// docs/specs/node-retire AC-9
#[test]
fn a_head_that_moved_is_left_and_the_rest_are_still_given_back_past_one_pass() {
    let shards = u32::try_from(GIVE_BACK_PER_PASS).expect("fits") + 3;
    let c = cluster();
    create_n(&c, "logs", "u1", shards);
    let one = node(&c, "node-1", 1, false, Some(takeover_of(&c, "node-1", 1)));
    let ids = ids_by_shard(shards, 1);
    for ids in &ids {
        write_id(&one.role, &ids[0]);
    }
    assert_eq!(
        one.role.hibernate_idle(0, now()).hibernated,
        shards as usize
    );
    // node-2 takes shard 0 while node-1's records say it sleeps.
    let two = node(&c, "node-2", 2, true, Some(takeover_of(&c, "node-2", 2)));
    write_id(&two.role, &ids[0][0]);
    let report = one.role.retire(now(), u64::MAX);
    assert_eq!(report.given_back, shards as usize - 1, "{report:?}");
    assert_eq!(report.busy, 1, "{report:?}");
    assert_eq!(one.role.hibernated(), 0, "no record is left behind");
    assert_eq!(owner(&c, 0).as_deref(), Some("node-2"));
}

// docs/specs/node-retire AC-10
#[test]
fn a_deadline_that_has_passed_leaves_every_shard_held_and_counts_them() {
    let c = cluster();
    create(&c, "logs", "u1");
    let one = node(&c, "node-1", 1, false, Some(takeover_of(&c, "node-1", 1)));
    assert_eq!(seq(&write(&one.role, "a")), 0);
    let before = requests(&c);
    let report = one.role.retire(now(), 0);
    assert_eq!(report.unfinished, 1, "{report:?}");
    assert_eq!((report.hibernated, report.given_back), (0, 0));
    assert_eq!(requests(&c), before, "an expired deadline asks nothing");
    assert_eq!(owner(&c, 0).as_deref(), Some("node-1"));
}
