//! docs/specs/writer-hibernation: a writer idle for long enough closes its
//! engine and keeps its head, and wakes at the same term on the next write.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::{BlobStore, S3Op};
use oss_cluster::heads::shard_head_key;
use oss_format::ShardHead;
use oss_node::writer::Takeover;
use oss_testkit::budget;
use oss_writer::bulk::ItemResult;
use oss_writer::index::Outcome;
use support::*;

const IDLE: u64 = 1_000;

fn real_now() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("after 1970")
            .as_millis(),
    )
    .expect("fits")
}

/// A clock `millis` on from now.
fn after(millis: u64) -> u64 {
    real_now() + millis
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

fn puts(c: &Cluster) -> u64 {
    c.store.meter().count(S3Op::Put)
}

/// node-1 with a takeover over its own log, an index, and `a` and `b` written.
fn writer() -> (Cluster, Node) {
    let c = cluster();
    create(&c, "logs", "u1");
    let store: Arc<dyn BlobStore> = c.store.clone();
    let one = node(
        &c,
        "node-1",
        1,
        false,
        Some(Takeover::from_leases(store, "", "node-1", 1, 1 << 20)),
    );
    assert_eq!(seq(&write(&one.role, "a")), 0);
    assert_eq!(seq(&write(&one.role, "b")), 1);
    (c, one)
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

fn outcome(result: &ItemResult) -> Outcome {
    match result {
        ItemResult::Written { written, .. } => written.outcome,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_pass_with_the_setting_off_hibernates_nothing_and_asks_nothing() {
    let (c, one) = writer();
    let before = requests(&c);
    // The setting's 0 is "never": the pass the timer runs reads it.
    let report = one.role.hibernate_due(after(10 * IDLE));
    assert_eq!(report.hibernated, 0);
    assert!(one.role.held("u1", 0));
    assert_eq!(requests(&c), before);
}

#[test]
fn an_idle_published_shard_sleeps_keeping_its_head_and_asking_nothing() {
    let (c, one) = writer();
    one.role.refresh(&info(), 0).expect("published");
    let term = head(&c).term;
    let before = requests(&c);
    let report = one.role.hibernate_idle(IDLE, after(2 * IDLE));
    assert_eq!(report.hibernated, 1, "{report:?}");
    budget::check(
        "writer.hibernation.idle_pass.requests",
        requests(&c) - before,
        "a pass that sleeps a published shard",
    );
    assert!(!one.role.held("u1", 0));
    assert!(one.role.asleep("u1", 0));
    assert_eq!(one.role.hibernated(), 1);
    let kept = head(&c);
    assert_eq!(kept.owner_node_id.as_deref(), Some("node-1"));
    assert_eq!(kept.term, term);
}

#[test]
fn a_shard_used_within_the_threshold_stays_open() {
    let (c, one) = writer();
    one.role.refresh(&info(), 0).expect("published");
    let report = one.role.hibernate_idle(IDLE, after(IDLE / 2));
    assert_eq!(report.hibernated, 0);
    assert!(one.role.held("u1", 0));
    drop(c);
}

#[test]
fn a_shard_with_unpublished_writes_is_published_then_sleeps() {
    let (c, one) = writer();
    // Nothing published yet: a and b are only in the log.
    let before = puts(&c);
    let report = one.role.hibernate_idle(IDLE, after(2 * IDLE));
    assert_eq!(report.hibernated, 1, "{report:?}");
    assert!(puts(&c) > before, "the pass published the commit");
    assert_eq!(registered_term(&c), 1);
    assert!(one.role.asleep("u1", 0));
}

#[test]
fn a_write_to_a_sleeping_shard_wakes_it_at_the_same_term_with_every_document() {
    let (c, one) = writer();
    assert_eq!(one.role.hibernate_idle(IDLE, after(2 * IDLE)).hibernated, 1);
    let term = head(&c).term;
    let swaps = puts(&c);
    // `a` is there, so it is an update; `c` is new.
    assert_eq!(outcome(&write(&one.role, "a")), Outcome::Updated);
    assert_eq!(outcome(&write(&one.role, "c")), Outcome::Created);
    assert!(one.role.held("u1", 0));
    assert!(!one.role.asleep("u1", 0));
    assert_eq!(head(&c).term, term, "no new term");
    assert!(puts(&c) > swaps, "the writes went to the log");
}

#[test]
fn waking_costs_what_the_table_says_and_less_than_taking_the_shard_over() {
    let (c, one) = writer();
    assert_eq!(one.role.hibernate_idle(IDLE, after(2 * IDLE)).hibernated, 1);
    let before = requests(&c);
    let puts_before = puts(&c);
    one.role.hold(&info(), 0).expect("wakes");
    let woke = requests(&c) - before;
    assert_eq!(puts(&c), puts_before, "a wake writes nothing");
    budget::check(
        "writer.hibernation.wake.requests",
        woke,
        "waking a hibernated shard whose commit holds one segment",
    );

    // The same shard after a restart of the process: a new role takes it
    // over from the head this node left.
    let (c2, one2) = writer();
    assert_eq!(
        one2.role.hibernate_idle(IDLE, after(2 * IDLE)).hibernated,
        1
    );
    let store: Arc<dyn BlobStore> = c2.store.clone();
    let again = node(
        &c2,
        "node-1",
        2,
        false,
        Some(Takeover::from_leases(store, "", "node-1", 2, 1 << 20)),
    );
    let before = requests(&c2);
    again.role.hold(&info(), 0).expect("taken");
    let taken = requests(&c2) - before;
    assert!(woke < taken, "woke in {woke}, taken over in {taken}");
}

#[test]
fn a_shard_whose_head_moved_while_it_slept_is_opened_the_ordinary_way() {
    let (c, one) = writer();
    assert_eq!(one.role.hibernate_idle(IDLE, after(2 * IDLE)).hibernated, 1);
    // node-2 sees node-1 as dead and takes the shard.
    let store: Arc<dyn BlobStore> = c.store.clone();
    let two = node(
        &c,
        "node-2",
        2,
        true,
        Some(Takeover::from_leases(store, "", "node-2", 2, 1 << 20)),
    );
    assert_eq!(seq(&write(&two.role, "x")), 2);
    let refused = one.role.hold(&info(), 0).expect_err("node-2 holds it");
    assert_eq!(refused.holder.as_deref(), Some("node-2"), "{refused:?}");
    assert!(!one.role.held("u1", 0));
    assert!(
        !one.role.asleep("u1", 0),
        "the record was used up by the ask"
    );
}

#[test]
fn a_write_racing_the_hibernation_lands() {
    let (c, one) = writer();
    drop(c);
    let role = Arc::clone(&one.role);
    let writer = std::thread::spawn(move || {
        for n in 0..40 {
            let result = write(&role, &format!("id-{n}"));
            // Quiet long enough for a pass to put it to sleep, so the next
            // write wakes it, with the passes running all the while.
            let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while !role.asleep("u1", 0) && std::time::Instant::now() < until {
                std::thread::yield_now();
            }
            assert_eq!(outcome(&result), Outcome::Created, "write {n}: {result:?}");
        }
    });
    let mut slept = 0;
    while !writer.is_finished() {
        // Idle after 20 ms: a shard just woken is not put straight back.
        slept += one.role.hibernate_idle(20, real_now()).hibernated;
    }
    writer.join().expect("every write landed");
    assert!(slept > 0, "the passes did put the shard to sleep");
}

#[test]
fn a_sleeping_shard_refreshes_without_a_request_and_a_get_says_it_is_not_open() {
    let (c, one) = writer();
    assert_eq!(one.role.hibernate_idle(IDLE, after(2 * IDLE)).hibernated, 1);
    let before = requests(&c);
    one.role.refresh(&info(), 0).expect("nothing to publish");
    assert_eq!(requests(&c), before);
    let err = one.role.get(&info(), 0, "a", true).expect_err("not open");
    assert!(err.contains("not open"), "{err}");
}
