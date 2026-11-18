//! docs/specs/writer-tail-push: the writer pushes acknowledged writes to
//! subscribed readers.

#![cfg(feature = "writer")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_cluster::lease::LeaseClock;
use oss_format::WalRecord;
use oss_node::push_sender::LinkError;
use oss_node::tail_keep::TailKeep;
use oss_node::tail_push::{MAX_QUEUED, TailLink, TailPush};
use oss_node::tail_serve::{ShardFacts, TailServe};
use oss_transport::actions::tail::{FetchRequest, NEW, PushRequest, PushResponse, ShardRef};

const U: &str = "uuid-a";

struct Facts(AtomicBool);

impl ShardFacts for Facts {
    fn writes(&self, _: &str, _: u32) -> bool {
        true
    }
    fn ownership_in_doubt(&self) -> bool {
        false
    }
    fn overlay_on(&self, _: &str, _: u32) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    fn max_seq_no(&self, _: &str, _: u32) -> i64 {
        0
    }
}

struct Clock;

impl LeaseClock for Clock {
    fn now_millis(&self) -> i64 {
        0
    }
    fn pause(&self, _: Duration) {}
}

type Hook = Box<dyn Fn(&str) + Send>;

/// Records every push and answers as scripted: a node absent from `fail`
/// answers, naming the shards in `unwanted[node]` unwanted.
#[derive(Default)]
struct Link {
    sent: Mutex<Vec<(String, PushRequest)>>,
    fail: Mutex<Vec<String>>,
    unwanted: Mutex<HashMap<String, Vec<i32>>>,
    during: Mutex<Option<Hook>>,
}

impl TailLink for Link {
    fn send(&self, target: &str, request: &PushRequest) -> Result<PushResponse, LinkError> {
        self.sent
            .lock()
            .unwrap()
            .push((target.to_owned(), request.clone()));
        if let Some(hook) = self.during.lock().unwrap().as_ref() {
            hook(target);
        }
        if self.fail.lock().unwrap().iter().any(|n| n == target) {
            return Err(LinkError);
        }
        let unwanted = self
            .unwanted
            .lock()
            .unwrap()
            .get(target)
            .cloned()
            .unwrap_or_default();
        Ok(PushResponse {
            unwanted: request
                .shards
                .iter()
                .filter(|(s, _)| unwanted.contains(&s.shard))
                .map(|(s, _)| s.clone())
                .collect(),
        })
    }
}

struct Rig {
    keep: Arc<TailKeep>,
    serve: Arc<TailServe>,
    link: Arc<Link>,
    facts: Arc<Facts>,
    push: TailPush,
}

fn rig() -> Rig {
    let facts = Arc::new(Facts(AtomicBool::new(true)));
    let keep = Arc::new(TailKeep::new());
    let serve = Arc::new(TailServe::new(keep.clone(), facts.clone(), Arc::new(Clock)));
    let link = Arc::new(Link::default());
    let push = TailPush::new(keep.clone(), serve.clone(), link.clone(), facts.clone());
    Rig {
        keep,
        serve,
        link,
        facts,
        push,
    }
}

fn rec(seq: i64) -> WalRecord<'static> {
    WalRecord::index(format!("d{seq}"), "{}".to_owned(), seq, 1, 1)
}

fn recs(range: std::ops::RangeInclusive<i64>) -> Vec<WalRecord<'static>> {
    range.map(rec).collect()
}

fn subscribe(r: &Rig, node: &str, shard: i32) -> i64 {
    r.serve
        .answer(&FetchRequest {
            shard: ShardRef {
                index_name: "logs".into(),
                index_uuid: U.into(),
                shard,
            },
            after_seq_no: 0,
            subscriber: node.into(),
            subscription: NEW,
            ..FetchRequest::default()
        })
        .subscription
}

fn seqs(request: &PushRequest, shard: i32) -> Vec<i64> {
    request
        .shards
        .iter()
        .filter(|(s, _)| s.shard == shard)
        .flat_map(|(_, records)| records.iter().map(|r| r.seq_no))
        .collect()
}

#[test]
fn acked_records_go_to_each_subscribed_node_in_one_request_in_order() {
    let r = rig();
    subscribe(&r, "n1", 0);
    subscribe(&r, "n1", 1);
    subscribe(&r, "n2", 0);
    r.push.acked("logs", U, 0, &recs(1..=2), true);
    r.push.acked("logs", U, 1, &recs(3..=3), true);
    r.push.acked("logs", U, 0, &recs(4..=4), true);
    r.push.flush();
    let sent = r.link.sent.lock().unwrap();
    assert_eq!(sent.len(), 2, "one request a node");
    let (node, one) = &sent[0];
    assert_eq!(node, "n1");
    assert_eq!(seqs(one, 0), [1, 2, 4]);
    assert_eq!(seqs(one, 1), [3]);
    assert_eq!(one.shards[0].0.index_name, "logs");
    let (node, two) = &sent[1];
    assert_eq!(node, "n2");
    assert_eq!(seqs(two, 0), [1, 2, 4]);
    assert_eq!(
        seqs(two, 1),
        Vec::<i64>::new(),
        "n2 does not follow shard 1"
    );
}

#[test]
fn no_subscriber_means_no_send_and_an_empty_queue_sends_nothing() {
    let r = rig();
    r.push.acked("logs", U, 0, &recs(1..=2), true);
    r.push.flush();
    r.push.flush();
    assert!(r.link.sent.lock().unwrap().is_empty());
    subscribe(&r, "n1", 0);
    r.push.acked("logs", U, 0, &recs(3..=3), true);
    r.push.flush();
    r.push.flush();
    assert_eq!(
        r.link.sent.lock().unwrap().len(),
        1,
        "a drained queue goes once"
    );
}

#[test]
fn an_answer_confirms_through_the_end_position_and_a_subscription_starts_at_its_own() {
    let r = rig();
    r.push.acked("logs", U, 0, &recs(1..=3), true);
    assert_eq!(subscribe(&r, "n1", 0), 0);
    assert_eq!(
        r.serve.confirmed(U, 0, "n1"),
        Some(3),
        "the position it subscribed at"
    );
    r.push.acked("logs", U, 0, &recs(4..=5), true);
    r.push.flush();
    assert_eq!(r.serve.confirmed(U, 0, "n1"), Some(5));
}

#[test]
fn answers_out_of_order_confirm_only_through_the_earliest_unanswered() {
    let r = rig();
    subscribe(&r, "n1", 0);
    let first = r.serve.targets(U, 0, 2);
    let second = r.serve.targets(U, 0, 5);
    let third = r.serve.targets(U, 0, 9);
    assert_eq!(
        (first[0].1.number, second[0].1.number, third[0].1.number),
        (0, 1, 2)
    );
    r.serve.confirm(U, 0, "n1", second[0].1);
    assert_eq!(r.serve.confirmed(U, 0, "n1"), Some(0), "0 is unanswered");
    r.serve.confirm(U, 0, "n1", second[0].1);
    r.serve.confirm(U, 0, "n1", first[0].1);
    assert_eq!(r.serve.confirmed(U, 0, "n1"), Some(5), "0 and 1 count");
    r.serve.confirm(U, 0, "n1", first[0].1);
    assert_eq!(
        r.serve.confirmed(U, 0, "n1"),
        Some(5),
        "answered twice counts once"
    );
    r.serve.confirm(U, 0, "n1", third[0].1);
    assert_eq!(r.serve.confirmed(U, 0, "n1"), Some(9));
}

#[test]
fn an_unwanted_shard_drops_that_shard_and_a_failed_send_drops_every_shard() {
    let r = rig();
    subscribe(&r, "n1", 0);
    subscribe(&r, "n1", 1);
    subscribe(&r, "n2", 0);
    subscribe(&r, "n2", 1);
    r.link.unwanted.lock().unwrap().insert("n1".into(), vec![1]);
    r.link.fail.lock().unwrap().push("n2".into());
    r.push.acked("logs", U, 0, &recs(1..=1), true);
    r.push.acked("logs", U, 1, &recs(2..=2), true);
    r.push.flush();
    assert_eq!(r.serve.subscribers_of(U, 0), ["n1"]);
    assert!(r.serve.subscribers_of(U, 1).is_empty());
    assert_eq!(r.serve.confirmed(U, 0, "n1"), Some(1));
}

#[test]
fn a_node_that_fetched_again_keeps_its_new_subscription() {
    let r = rig();
    subscribe(&r, "n1", 0);
    let old = r.serve.targets(U, 0, 1)[0].1;
    let fresh = subscribe(&r, "n1", 0);
    assert_ne!(fresh, old.id);
    r.serve.drop_sent(U, 0, "n1", old);
    assert_eq!(
        r.serve.subscribers_of(U, 0),
        ["n1"],
        "an old push says nothing"
    );
    r.serve.confirm(U, 0, "n1", old);
    assert_eq!(
        r.serve.confirmed(U, 0, "n1"),
        Some(0),
        "an old answer confirms nothing of the new"
    );
    let now = r.serve.targets(U, 0, 7)[0].1;
    assert_eq!((now.id, now.number), (fresh, 0), "numbers restart");
    r.serve.drop_sent(U, 0, "n1", now);
    assert!(r.serve.subscribers_of(U, 0).is_empty());
}

#[test]
fn a_write_not_to_be_pushed_queues_nothing_and_off_drops_the_shard() {
    let r = rig();
    subscribe(&r, "n1", 0);
    r.push.acked("logs", U, 0, &recs(1..=2), false);
    r.push.flush();
    assert!(r.link.sent.lock().unwrap().is_empty());
    assert!(r.keep.keeps(U, 0));
    r.push.acked("logs", U, 0, &[], true);
    assert!(r.keep.keeps(U, 0), "an empty batch does nothing");
    assert_eq!(r.serve.subscribers_of(U, 0), ["n1"]);
    r.facts.0.store(false, Ordering::SeqCst);
    r.push.acked("logs", U, 0, &recs(3..=3), true);
    assert!(!r.keep.keeps(U, 0));
    assert!(r.serve.subscribers_of(U, 0).is_empty());
    r.push.flush();
    assert!(r.link.sent.lock().unwrap().is_empty());
}

#[test]
fn past_the_queue_bound_a_shards_subscribers_and_queue_are_dropped() {
    let r = rig();
    subscribe(&r, "n1", 0);
    subscribe(&r, "n1", 1);
    let n = MAX_QUEUED as i64;
    r.push.acked("logs", U, 1, &recs(1..=1), true);
    r.push.acked("logs", U, 0, &recs(1..=n), true);
    assert_eq!(
        r.serve.subscribers_of(U, 0),
        ["n1"],
        "at the bound is not over it"
    );
    r.push.acked("logs", U, 0, &recs(n + 1..=n + 1), true);
    assert!(r.serve.subscribers_of(U, 0).is_empty());
    assert_eq!(
        r.serve.subscribers_of(U, 1),
        ["n1"],
        "another shard is untouched"
    );
    r.push.flush();
    let sent = r.link.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(seqs(&sent[0].1, 0), Vec::<i64>::new(), "its queue is gone");
    assert_eq!(seqs(&sent[0].1, 1), [1]);
}

#[test]
fn the_default_bound_is_twenty_thousand_records() {
    assert_eq!(MAX_QUEUED, 20_000);
}

#[test]
fn an_answer_to_an_old_push_does_not_touch_a_subscription_made_since() {
    let r = rig();
    let old = subscribe(&r, "n1", 0);
    r.link.unwanted.lock().unwrap().insert("n1".into(), vec![0]);
    let serve = r.serve.clone();
    let renewed = Arc::new(Mutex::new(-1));
    let seen = renewed.clone();
    *r.link.during.lock().unwrap() = Some(Box::new(move |node: &str| {
        // The node fetches again while its push is in flight.
        let again = serve.answer(&FetchRequest {
            shard: ShardRef {
                index_name: "logs".into(),
                index_uuid: U.into(),
                shard: 0,
            },
            subscriber: node.into(),
            subscription: NEW,
            ..FetchRequest::default()
        });
        *seen.lock().unwrap() = again.subscription;
    }));
    r.push.acked("logs", U, 0, &recs(1..=1), true);
    r.push.flush();
    assert_ne!(*renewed.lock().unwrap(), old);
    assert_eq!(
        r.serve.subscribers_of(U, 0),
        ["n1"],
        "unwanted was for the old push"
    );
    assert_eq!(r.serve.confirmed(U, 0, "n1"), Some(1));
}

#[test]
fn concurrent_acks_and_flushes_deliver_every_record_once_and_confirm_through_the_last() {
    let r = Arc::new(rig());
    subscribe(&r, "n1", 0);
    let threads: Vec<_> = (0..4_i64)
        .map(|t| {
            let r = r.clone();
            std::thread::spawn(move || {
                for i in 0..100_i64 {
                    let seq = t * 1_000 + i;
                    r.push.acked("logs", U, 0, &[rec(seq)], true);
                    if i % 7 == 0 {
                        r.push.flush();
                    }
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    r.push.flush();
    let mut got: Vec<i64> = r
        .link
        .sent
        .lock()
        .unwrap()
        .iter()
        .flat_map(|(_, request)| seqs(request, 0))
        .collect();
    got.sort_unstable();
    let mut want: Vec<i64> = (0..4_i64)
        .flat_map(|t| (0..100).map(move |i| t * 1_000 + i))
        .collect();
    want.sort_unstable();
    assert_eq!(got, want);
    assert_eq!(r.serve.confirmed(U, 0, "n1"), Some(400));
}

#[test]
fn turning_a_shard_off_discards_its_queued_writes_and_only_its_own() {
    let r = rig();
    for (uuid, shard) in [(U, 0), (U, 1), ("other", 0)] {
        r.serve.answer(&FetchRequest {
            shard: ShardRef {
                index_name: "logs".into(),
                index_uuid: uuid.into(),
                shard,
            },
            subscriber: "n1".into(),
            subscription: NEW,
            ..FetchRequest::default()
        });
        r.push.acked("logs", uuid, shard as u32, &recs(1..=1), true);
    }
    r.facts.0.store(false, Ordering::SeqCst);
    r.push.acked("logs", U, 0, &recs(2..=2), true);
    r.facts.0.store(true, Ordering::SeqCst);
    r.push.flush();
    let sent = r.link.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    let shards: Vec<(&str, i32)> = sent[0]
        .1
        .shards
        .iter()
        .map(|(s, _)| (s.index_uuid.as_str(), s.shard))
        .collect();
    assert_eq!(
        shards,
        [(U, 1), ("other", 0)],
        "(U, 0) is gone, the rest stay"
    );
}
