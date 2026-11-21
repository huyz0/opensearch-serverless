//! docs/specs/writer-tail-await: a refreshing write waits for the nodes a
//! shard is pushed to.

#![cfg(feature = "writer")]

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_cluster::lease::LeaseClock;
use oss_format::WalRecord;
use oss_node::push_sender::LinkError;
use oss_node::tail_keep::TailKeep;
use oss_node::tail_push::{TailLink, TailPush};
use oss_node::tail_serve::{ShardFacts, TailServe};
use oss_transport::actions::tail::{FetchRequest, NEW, PushRequest, PushResponse, ShardRef};

const U: &str = "uuid-a";
const WAIT: Duration = Duration::from_millis(200);

struct Facts;

impl ShardFacts for Facts {
    fn writes(&self, _: &str, _: u32) -> bool {
        true
    }
    fn ownership_in_doubt(&self) -> bool {
        false
    }
    fn overlay_on(&self, _: &str, _: u32) -> bool {
        true
    }
    fn max_seq_no(&self, _: &str, _: u32) -> i64 {
        0
    }
}

/// A clock that moves only when something pauses on it.
#[derive(Default)]
struct Clock(AtomicI64);

impl LeaseClock for Clock {
    fn now_millis(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
    fn pause(&self, d: Duration) {
        self.0.fetch_add(d.as_millis() as i64, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Link {
    sent: AtomicUsize,
    fail: Mutex<Vec<String>>,
    unwanted: Mutex<Vec<String>>,
}

impl TailLink for Link {
    fn send(&self, target: &str, request: &PushRequest) -> Result<PushResponse, LinkError> {
        self.sent.fetch_add(1, Ordering::SeqCst);
        if self.fail.lock().unwrap().iter().any(|n| n == target) {
            return Err(LinkError);
        }
        let unwanted = self.unwanted.lock().unwrap().iter().any(|n| n == target);
        Ok(PushResponse {
            unwanted: if unwanted {
                request.shards.iter().map(|(s, _)| s.clone()).collect()
            } else {
                Vec::new()
            },
        })
    }
}

struct Rig {
    serve: Arc<TailServe>,
    link: Arc<Link>,
    clock: Arc<Clock>,
    push: TailPush,
}

fn rig() -> Rig {
    let facts = Arc::new(Facts);
    let keep = Arc::new(TailKeep::new());
    let clock = Arc::new(Clock::default());
    let serve = Arc::new(TailServe::new(keep.clone(), facts.clone(), clock.clone()));
    let link = Arc::new(Link::default());
    let push = TailPush::new(keep, serve.clone(), link.clone(), facts);
    Rig {
        serve,
        link,
        clock,
        push,
    }
}

fn recs(range: std::ops::RangeInclusive<i64>) -> Vec<WalRecord<'static>> {
    range
        .map(|s| WalRecord::index(format!("d{s}"), "{}".to_owned(), s, 1, 1))
        .collect()
}

fn subscribe(r: &Rig, node: &str) {
    let answer = r.serve.answer(&FetchRequest {
        shard: ShardRef {
            index_name: "logs".into(),
            index_uuid: U.into(),
            shard: 0,
        },
        subscriber: node.into(),
        subscription: NEW,
        ..FetchRequest::default()
    });
    assert!(answer.subscription >= 0);
}

fn ack(r: &Rig, seqs: std::ops::RangeInclusive<i64>) {
    r.push.acked("logs", U, 0, &recs(seqs), true);
}

#[test]
fn a_shard_nothing_is_kept_for_waits_for_nothing() {
    let r = rig();
    assert!(r.push.await_search_nodes(U, 0, WAIT, false));
    assert_eq!(r.link.sent.load(Ordering::SeqCst), 0);
    assert_eq!(r.clock.now_millis(), 0);
}

#[test]
fn the_wait_pushes_at_once_and_ends_when_every_node_answered() {
    let r = rig();
    ack(&r, 1..=1);
    subscribe(&r, "n1");
    subscribe(&r, "n2");
    ack(&r, 2..=3);
    assert_eq!(r.serve.confirmed(U, 0, "n1"), Some(1));
    assert!(r.push.await_search_nodes(U, 0, WAIT, false));
    // One request a node, sent by the wait and not by a timer.
    assert_eq!(r.link.sent.load(Ordering::SeqCst), 2);
    assert_eq!(r.serve.confirmed(U, 0, "n1"), Some(3));
    assert_eq!(r.serve.confirmed(U, 0, "n2"), Some(3));
    // Nothing behind: nothing to send or wait for.
    assert!(r.push.await_search_nodes(U, 0, WAIT, false));
    assert_eq!(r.link.sent.load(Ordering::SeqCst), 2);
}

#[test]
fn a_lost_push_is_not_reached_and_an_unwanted_answer_is() {
    let r = rig();
    ack(&r, 1..=1);
    subscribe(&r, "n1");
    subscribe(&r, "n2");
    ack(&r, 2..=2);
    r.link.fail.lock().unwrap().push("n1".into());
    assert!(!r.push.await_search_nodes(U, 0, WAIT, false));
    assert_eq!(r.serve.subscribers_of(U, 0), ["n2"]);
    // A node that says it has no overlay serves nothing from the push: not lost.
    let r = rig();
    ack(&r, 1..=1);
    subscribe(&r, "n1");
    ack(&r, 2..=2);
    r.link.unwanted.lock().unwrap().push("n1".into());
    assert!(r.push.await_search_nodes(U, 0, WAIT, false));
    assert!(r.serve.subscribers_of(U, 0).is_empty());
}

#[test]
fn a_wait_times_out_and_ends_when_the_shard_is_let_go() {
    let r = rig();
    ack(&r, 1..=1);
    subscribe(&r, "n1");
    // Behind a position nothing was queued for: nobody will confirm it.
    let id = r.serve.open_wait(U, 0, 99).expect("n1 is behind");
    let started = std::time::Instant::now();
    assert!(!r.serve.wait(id, Duration::from_millis(30)));
    assert!(started.elapsed() >= Duration::from_millis(25));
    let id = r.serve.open_wait(U, 0, 99).expect("still behind");
    r.serve.released(U, 0);
    assert!(!r.serve.wait(id, Duration::from_secs(5)));
    // Nobody to wait for.
    assert_eq!(r.serve.open_wait(U, 0, 99), None);
}

#[test]
fn grants_wait_out_a_shard_this_node_only_just_began_keeping() {
    let r = rig();
    ack(&r, 1..=1);
    // Without grants, nothing waits.
    assert!(r.push.await_search_nodes(U, 0, WAIT, false));
    assert_eq!(r.clock.now_millis(), 0);
    // With them, a grant an earlier writer handed out may still run: 600 ms
    // from when this node began keeping the shard.
    assert!(r.push.await_search_nodes(U, 0, WAIT, true));
    assert_eq!(r.clock.now_millis(), 600);
    // Later it has run out.
    assert!(r.push.await_search_nodes(U, 0, WAIT, true));
    assert_eq!(r.clock.now_millis(), 600);
}

#[test]
fn grants_wait_out_the_grant_of_a_node_no_longer_pushed_to() {
    let r = rig();
    ack(&r, 1..=1);
    r.clock.pause(Duration::from_millis(700));
    subscribe(&r, "n1");
    ack(&r, 2..=2);
    r.link.fail.lock().unwrap().push("n1".into());
    // The push is lost: the write is not reached, and n1's grant, 600 ms from
    // its fetch at 700, may still be answering searches.
    assert!(!r.push.await_search_nodes(U, 0, WAIT, true));
    r.clock.pause(Duration::from_millis(100));
    assert!(r.push.await_search_nodes(U, 0, WAIT, true));
    assert_eq!(r.clock.now_millis(), 1300);
    // An answer that says no overlay leaves no grant to wait out.
    let r = rig();
    ack(&r, 1..=1);
    r.clock.pause(Duration::from_millis(700));
    subscribe(&r, "n1");
    ack(&r, 2..=2);
    r.link.unwanted.lock().unwrap().push("n1".into());
    assert!(r.push.await_search_nodes(U, 0, WAIT, true));
    assert_eq!(r.clock.now_millis(), 700);
}
