//! docs/specs/writer-tail-fetch: the writer answers a reader's tail fetch.

#![cfg(feature = "writer")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use oss_cluster::lease::LeaseClock;
use oss_format::WalRecord;
use oss_node::tail_keep::TailKeep;
use oss_node::tail_serve::{GRANT_MILLIS, ShardFacts, TailServe};
use oss_transport::actions::tail::{CHECK, FetchRequest, NEW, NONE, ShardRef};

const U: &str = "uuid-a";

struct Facts {
    writes: AtomicBool,
    doubt: AtomicBool,
    on: AtomicBool,
    max: AtomicI64,
}

impl Facts {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            writes: AtomicBool::new(true),
            doubt: AtomicBool::new(false),
            on: AtomicBool::new(true),
            max: AtomicI64::new(5),
        })
    }
}

impl ShardFacts for Facts {
    fn writes(&self, _: &str, _: u32) -> bool {
        self.writes.load(Ordering::SeqCst)
    }
    fn ownership_in_doubt(&self) -> bool {
        self.doubt.load(Ordering::SeqCst)
    }
    fn overlay_on(&self, _: &str, _: u32) -> bool {
        self.on.load(Ordering::SeqCst)
    }
    fn max_seq_no(&self, _: &str, _: u32) -> i64 {
        self.max.load(Ordering::SeqCst)
    }
}

struct Clock(AtomicI64);

impl LeaseClock for Clock {
    fn now_millis(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
    fn pause(&self, _: Duration) {}
}

struct Rig {
    facts: Arc<Facts>,
    keep: Arc<TailKeep>,
    clock: Arc<Clock>,
    serve: TailServe,
}

fn rig() -> Rig {
    let facts = Facts::new();
    let keep = Arc::new(TailKeep::new());
    let clock = Arc::new(Clock(AtomicI64::new(1_000)));
    let serve = TailServe::new(keep.clone(), facts.clone(), clock.clone());
    Rig {
        facts,
        keep,
        clock,
        serve,
    }
}

fn rec(seq: i64) -> WalRecord<'static> {
    WalRecord::index(format!("d{seq}"), "{}".to_owned(), seq, 1, 1)
}

fn ask(node: &str, after: i64, subscription: i64) -> FetchRequest {
    FetchRequest {
        shard: ShardRef {
            index_name: "logs".into(),
            index_uuid: U.into(),
            shard: 0,
        },
        after_seq_no: after,
        subscriber: node.into(),
        subscription,
        ..FetchRequest::default()
    }
}

fn seqs(records: &[oss_transport::actions::tail::Record]) -> Vec<i64> {
    records.iter().map(|r| r.seq_no).collect()
}

#[test]
fn a_shard_not_written_here_or_in_doubt_answers_only_the_unknown_claim() {
    let r = rig();
    r.keep.acked(U, 0, &[rec(6)], true);
    r.facts.writes.store(false, Ordering::SeqCst);
    let a = r.serve.answer(&ask("n1", 0, NEW));
    assert_eq!(
        (a.records.len(), a.subscription, a.complete),
        (0, NONE, false)
    );
    assert_eq!(a.claimed_through, i64::MAX);
    r.facts.writes.store(true, Ordering::SeqCst);
    r.facts.doubt.store(true, Ordering::SeqCst);
    let a = r.serve.answer(&ask("n1", 0, NEW));
    assert_eq!(
        (a.records.len(), a.subscription, a.complete),
        (0, NONE, false)
    );
    assert_eq!(a.claimed_through, i64::MAX);
    assert!(r.serve.subscribers_of(U, 0).is_empty());
}

#[test]
fn off_a_check_and_a_gap_answer_the_claim_and_subscribe_nobody() {
    let r = rig();
    r.keep.acked(U, 0, &[rec(6)], true);
    r.facts.on.store(false, Ordering::SeqCst);
    let a = r.serve.answer(&ask("n1", 0, NEW));
    assert_eq!(
        (a.records.len(), a.subscription, a.complete),
        (0, NONE, false)
    );
    assert_eq!(a.claimed_through, 5, "the shard's highest, the first time");
    r.facts.on.store(true, Ordering::SeqCst);
    let a = r.serve.answer(&ask("n1", 0, CHECK));
    assert_eq!((a.records.len(), a.subscription), (0, NONE));
    assert_eq!(a.claimed_through, 5);

    let small = TailKeep::with_limits(oss_node::tail_keep::Limits {
        shard_records: 1,
        ..oss_node::tail_keep::Limits::default()
    });
    let keep = Arc::new(small);
    let serve = TailServe::new(keep.clone(), r.facts.clone(), r.clock.clone());
    keep.acked(U, 0, &[rec(6), rec(7)], true);
    let a = serve.answer(&ask("n1", 5, NEW));
    assert_eq!(
        (a.records.len(), a.subscription),
        (0, NONE),
        "6 was dropped"
    );
    let a = serve.answer(&ask("n1", 6, NEW));
    assert_eq!(seqs(&a.records), [7], "a commit at 6 has what was dropped");
    assert!(r.serve.subscribers_of(U, 0).is_empty());
}

#[test]
fn a_new_fetch_is_subscribed_before_it_reads_and_ids_count_from_zero() {
    let r = rig();
    r.keep.acked(U, 0, &[rec(6), rec(7), rec(8)], true);
    let a = r.serve.answer(&ask("n1", 6, NEW));
    assert_eq!(seqs(&a.records), [7, 8]);
    assert_eq!(a.subscription, 0);
    assert!(a.complete, "the floor is 5 and nothing was dropped");
    assert_eq!(a.claimed_through, 5);
    let b = r.serve.answer(&ask("n2", 3, NEW));
    assert_eq!(b.subscription, 1);
    assert!(!b.complete, "below the floor of 5");
    let mut nodes = r.serve.subscribers_of(U, 0);
    nodes.sort();
    assert_eq!(nodes, ["n1", "n2"]);
}

#[test]
fn a_fetch_on_a_shard_nothing_was_acked_for_begins_the_keep_at_its_highest() {
    let r = rig();
    let a = r.serve.answer(&ask("n1", 5, NEW));
    assert!(a.records.is_empty());
    assert!(a.complete);
    assert_eq!(a.subscription, 0);
    assert!(r.keep.keeps(U, 0));
    r.keep.acked(U, 0, &[rec(6)], true);
    assert_eq!(r.keep.kept_after(U, 0, 5).len(), 1);
}

#[test]
fn naming_the_held_subscription_renews_it_and_anything_else_subscribes_anew() {
    let r = rig();
    r.keep.acked(U, 0, &[rec(6)], true);
    let first = r.serve.answer(&ask("n1", 5, NEW));
    assert_eq!(first.subscription, 0);
    r.clock.0.store(1_300, Ordering::SeqCst);
    let renewed = r.serve.answer(&ask("n1", 5, first.subscription));
    assert_eq!(renewed.subscription, 0);
    assert!(renewed.complete);
    assert!(renewed.records.is_empty(), "a renewal needs nothing else");
    assert_eq!(renewed.claimed_through, 5);
    let stranger = r.serve.answer(&ask("n2", 5, first.subscription));
    assert_eq!(stranger.subscription, 1, "n2 does not hold n1's id");
    assert_eq!(stranger.records.len(), 1);
    assert_eq!(r.serve.grant_until(U, 0, "n1"), Some(1_300 + GRANT_MILLIS));
    r.clock.0.store(1_000, Ordering::SeqCst);
    r.serve.answer(&ask("n1", 5, 0));
    assert_eq!(
        r.serve.grant_until(U, 0, "n1"),
        Some(1_300 + GRANT_MILLIS),
        "a renewal never shortens"
    );
    let other = r.serve.answer(&ask("n1", 5, 9));
    assert_eq!(other.subscription, 2, "an id it does not hold is new");
    assert_eq!(
        r.serve.grant_until(U, 0, "n1"),
        Some(1_000 + GRANT_MILLIS),
        "the new one's grant"
    );
    assert_eq!(other.records.len(), 1);
    let fresh = r.serve.answer(&ask("n1", 5, NEW));
    assert_eq!(fresh.subscription, 3);
}

#[test]
fn a_grant_is_six_hundred_milliseconds_from_now() {
    assert_eq!(GRANT_MILLIS, 600);
    let r = rig();
    r.serve.answer(&ask("n1", 5, NEW));
    assert_eq!(r.serve.grant_until(U, 0, "n1"), Some(1_600));
    assert_eq!(r.serve.grant_until(U, 0, "n2"), None);
}

#[test]
fn the_claim_is_the_highest_the_first_time_is_raised_and_never_lowered() {
    let r = rig();
    assert_eq!(r.serve.claimed_through(U, 0), 5);
    r.facts.max.store(9, Ordering::SeqCst);
    assert_eq!(r.serve.claimed_through(U, 0), 5, "kept from the first");
    r.serve.claim(U, 0);
    assert_eq!(r.serve.claimed_through(U, 0), 9);
    r.facts.max.store(2, Ordering::SeqCst);
    r.serve.claim(U, 0);
    assert_eq!(r.serve.claimed_through(U, 0), 9, "never lowered");
    r.serve.released(U, 0);
    assert_eq!(r.serve.claimed_through(U, 0), 2, "released forgets it");
}

#[test]
fn unsubscribe_removes_one_node_and_released_removes_all_and_the_keep() {
    let r = rig();
    r.serve.answer(&ask("n1", 5, NEW));
    r.serve.answer(&ask("n2", 5, NEW));
    r.serve.unsubscribe(U, 0, "n1");
    assert_eq!(r.serve.subscribers_of(U, 0), ["n2"]);
    r.serve.unsubscribe(U, 0, "nobody");
    r.serve.released(U, 0);
    assert!(r.serve.subscribers_of(U, 0).is_empty());
    assert!(!r.keep.keeps(U, 0));
}

#[test]
fn a_keep_begun_by_a_fetch_raises_a_lower_claim_to_its_floor() {
    let r = rig();
    r.facts.max.store(3, Ordering::SeqCst);
    assert_eq!(r.serve.claimed_through(U, 0), 3);
    r.facts.max.store(5, Ordering::SeqCst);
    let a = r.serve.answer(&ask("n1", 5, NEW));
    assert_eq!(a.claimed_through, 5, "the floor, not the earlier claim");
    assert_eq!(r.serve.claimed_through(U, 0), 5);
}
