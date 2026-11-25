//! docs/specs/write-pipeline: groups of one shard wait on the node log
//! together, finish in the order they staged, and a failure of one fails the
//! groups behind it.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oss_blob::emulator::faults::Latency;
use oss_blob::{BlobStore, S3Op};
use oss_format::WalRecord;
use oss_node::writer::{TailSink, Takeover};
use oss_testkit::budget;
use oss_writer::bulk::ItemResult;
use oss_writer::index::Outcome;
use support::*;

const PUT: Duration = Duration::from_millis(150);

fn landed(r: &ItemResult) -> bool {
    matches!(r, ItemResult::Written { .. })
}

/// A node over an index `logs`, its log's PUTs slowed to `PUT`, one write
/// made so its epoch is open.
fn rig(depth: usize) -> (Cluster, Node) {
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
    one.role.set_pipeline_depth(depth);
    assert_eq!(seq(&write(&one.role, "warm")), 0);
    c.store.set_latency(
        S3Op::Put,
        Latency {
            base: PUT,
            jitter: Duration::ZERO,
        },
    );
    (c, one)
}

fn puts(c: &Cluster) -> u64 {
    c.store.meter().count(S3Op::Put)
}

/// `ids` written at once, staggered so each stages after the one before.
fn together(one: &Node, ids: &[&str]) -> Vec<ItemResult> {
    std::thread::scope(|s| {
        let handles: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(n, id)| {
                let role = Arc::clone(&one.role);
                s.spawn(move || {
                    std::thread::sleep(Duration::from_millis(10 * u64::try_from(n).unwrap_or(0)));
                    write(&role, id)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("ends"))
            .collect()
    })
}

// AC-3
#[test]
fn groups_in_flight_together_all_land_in_one_put() {
    let (c, one) = rig(8);
    let before = puts(&c);
    let started = Instant::now();
    let results = together(&one, &["a", "b", "c", "d"]);
    assert!(results.iter().all(landed), "{results:?}");
    // One request a window, not a window each: a window here is 0 ms, so
    // the groups still ride PUTs they overlap.
    assert!(
        started.elapsed() < PUT * 3,
        "four groups took {:?}",
        started.elapsed()
    );
    budget::check(
        "write.pipelined.requests",
        puts(&c) - before,
        "four groups of one shard at depth 8",
    );
    let mut seqs: Vec<i64> = results.iter().map(seq).collect();
    seqs.sort_unstable();
    assert_eq!(seqs, [1, 2, 3, 4], "each its own number");
    assert_eq!(one.role.max_seq_no("u1", 0), 4);
}

// AC-2
#[test]
fn at_depth_one_the_groups_of_a_shard_go_one_at_a_time() {
    let (_c, one) = rig(1);
    let started = Instant::now();
    let results = together(&one, &["a", "b", "c", "d"]);
    assert!(results.iter().all(landed), "{results:?}");
    assert!(
        started.elapsed() >= PUT * 4,
        "four groups took {:?}",
        started.elapsed()
    );
}

// AC-4
#[test]
fn a_failed_put_fails_every_group_in_flight_and_the_shard_is_released_by_the_last() {
    let (c, one) = rig(8);
    c.store.fail_next(S3Op::Put, 1);
    let results = together(&one, &["a", "b", "c"]);
    assert!(
        results.iter().all(|r| !landed(r)),
        "none acknowledged: {results:?}"
    );
    assert!(!one.role.held("u1", 0), "released once all were out");
    // The shard reopens from the log; the refused writes are not promised.
    c.store.set_latency(S3Op::Put, Latency::default());
    // A client's retry: the windows queued behind the failed one may have
    // ended the epoch the shard reopens into once more.
    let landed_again = (0..3).any(|_| landed(&write(&one.role, "after")));
    assert!(landed_again, "the shard takes writes again");
    assert!(one.role.held("u1", 0));
}

// AC-4
#[test]
fn a_group_staged_after_a_failure_is_refused_not_hung() {
    let (c, one) = rig(8);
    c.store.fail_next(S3Op::Put, 1);
    let first = {
        let role = Arc::clone(&one.role);
        std::thread::spawn(move || write(&role, "a"))
    };
    std::thread::sleep(PUT / 3);
    let second = write(&one.role, "b");
    assert!(!landed(&first.join().expect("ends")));
    assert!(!landed(&second), "{second:?}");
}

// AC-5
#[test]
fn a_retire_while_groups_are_in_flight_loses_none_of_the_acknowledged() {
    let (_c, one) = rig(8);
    let writers: Vec<_> = (0..4)
        .map(|n| {
            let role = Arc::clone(&one.role);
            std::thread::spawn(move || write(&role, &format!("w{n}")))
        })
        .collect();
    std::thread::sleep(PUT / 3);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_millis();
    let now = u64::try_from(now).expect("fits");
    one.role.retire(now, now + 30_000);
    let results: Vec<_> = writers
        .into_iter()
        .map(|h| h.join().expect("ends"))
        .collect();
    assert!(results.iter().all(landed), "{results:?}");
    // Every one is there after the shard is woken.
    for n in 0..4 {
        match write(&one.role, &format!("w{n}")) {
            ItemResult::Written { written, .. } => {
                assert_eq!(written.outcome, Outcome::Updated, "w{n} was kept");
            }
            other => panic!("{other:?}"),
        }
    }
}

struct Panics;

impl TailSink for Panics {
    fn acked(&self, _: &str, _: &str, _: u32, _: &[WalRecord<'static>]) {
        panic!("a sink that panics");
    }
    fn published(&self, _: &str, _: u32, _: i64) {}
    fn released(&self, _: &str, _: u32) {}
}

// AC-6
#[test]
fn a_panic_while_acknowledging_fails_the_slot_and_hangs_nothing() {
    let (_c, one) = rig(8);
    assert!(one.role.set_tail_sink(Arc::new(Panics)));
    let tries = Arc::new(AtomicUsize::new(0));
    let results = Arc::new(Mutex::new(Vec::new()));
    std::thread::scope(|s| {
        for id in ["a", "b", "c"] {
            let (role, tries, results) = (
                Arc::clone(&one.role),
                Arc::clone(&tries),
                Arc::clone(&results),
            );
            s.spawn(move || {
                let out =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write(&role, id)));
                tries.fetch_add(1, Ordering::Relaxed);
                results.lock().expect("lock").push(out.is_ok());
            });
        }
    });
    assert_eq!(tries.load(Ordering::Relaxed), 3, "every thread came out");
    assert!(!one.role.held("u1", 0), "the slot was released");
}

// AC-9
#[test]
fn a_refresh_while_groups_are_in_flight_waits_for_them_and_returns() {
    let (_c, one) = rig(8);
    let finished = Arc::new(AtomicUsize::new(0));
    let writers: Vec<_> = (0..3)
        .map(|n| {
            let role = Arc::clone(&one.role);
            let finished = Arc::clone(&finished);
            std::thread::spawn(move || {
                let result = write(&role, &format!("r{n}"));
                finished.fetch_add(1, Ordering::SeqCst);
                result
            })
        })
        .collect();
    std::thread::sleep(PUT / 3);
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "still waiting for the log"
    );
    let asked = Instant::now();
    one.role.refresh_owned(&info(), 0).expect("refreshes");
    // The publish waited for the window the groups were in, about what was
    // left of its PUT.
    assert!(asked.elapsed() >= PUT / 2, "{:?}", asked.elapsed());
    let results: Vec<_> = writers
        .into_iter()
        .map(|h| h.join().expect("ends"))
        .collect();
    assert!(results.iter().all(landed), "{results:?}");
}

// AC-10
#[test]
fn a_refresh_after_a_failed_window_does_not_hang_and_the_shard_goes_on() {
    let (c, one) = rig(8);
    c.store.fail_next(S3Op::Put, 1);
    let writers: Vec<_> = (0..3)
        .map(|n| {
            let role = Arc::clone(&one.role);
            std::thread::spawn(move || write(&role, &format!("f{n}")))
        })
        .collect();
    std::thread::sleep(PUT / 3);
    // Ok or an error is both fine: it must come back, and publish nothing
    // unlogged.
    let _ = one.role.refresh_owned(&info(), 0);
    for h in writers {
        assert!(!landed(&h.join().expect("ends")));
    }
    c.store.set_latency(S3Op::Put, Latency::default());
    let again = (0..3).any(|_| landed(&write(&one.role, "after")));
    assert!(again, "the shard takes writes again");
}

// AC-11
#[test]
fn a_depth_of_two_bounds_the_groups_of_a_shard_in_flight() {
    let (c, one) = rig(2);
    let before = puts(&c);
    let results = together(&one, &["a", "b", "c", "d", "e", "f"]);
    assert!(results.iter().all(landed), "{results:?}");
    // Six groups, two at a time: at least three windows one after another.
    let windows = puts(&c) - before;
    assert!(
        windows >= 3,
        "{windows} PUTs: depth 2 held six groups together"
    );
}

// AC-12
#[test]
fn a_node_that_takes_over_after_a_pipelined_node_keeps_every_acknowledged_write() {
    let (c, one) = rig(8);
    let results = together(&one, &["a", "b", "c", "d"]);
    assert!(results.iter().all(landed), "{results:?}");
    let top = results.iter().map(seq).max().expect("some");
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    c.store.set_latency(S3Op::Put, Latency::default());
    let two = node(&c, "node-2", 2, true, Some(take));
    // Every acknowledged write is found again, and the next number is past
    // the highest acknowledged.
    for id in ["a", "b", "c", "d"] {
        match write(&two.role, id) {
            ItemResult::Written { written, .. } => {
                assert_eq!(written.outcome, Outcome::Updated, "{id} was kept");
                assert!(written.seq_no > top, "{id} numbered past {top}");
            }
            other => panic!("{other:?}"),
        }
    }
}
