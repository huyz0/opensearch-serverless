//! docs/specs/request-cost-window: the writers that share a commit window
//! share the window's store requests among those that asked to see them.

mod common;

use std::sync::{Arc, Barrier};
use std::time::Duration;

use common::Hooked;
use common::committer::{rec, rig_on, shard, window};
use oss_blob::cost::{self, RequestCost, Scope};
use oss_blob::usage::{Classifier, Ledger};
use oss_blob::{BlobStore, Emulator, S3Op, Shared, UsageStore};
use oss_writer::log::{HedgeOutcome, NodeLogStore, ThreadPerPut};

struct NoIndex;

impl Classifier for NoIndex {
    fn index_of<'a>(&self, _: &'a str) -> Option<&'a str> {
        None
    }
}

/// A committer whose log store counts into request scopes.
fn rig() -> common::committer::Rig {
    let emulator = Arc::new(Emulator::new(1 << 30, 7));
    let counting = Arc::new(UsageStore::new(
        Emulator::new(1 << 30, 7),
        Arc::new(Ledger::new(8)),
        Arc::new(NoIndex),
    ));
    let log = NodeLogStore::new(counting as Arc<dyn BlobStore>, "", "node-1").expect("names");
    rig_on(emulator, log, window(300))
}

/// Writers that each commit one record of `ids[n]` in the one window, the
/// ones that `ask` with a request scope of their own; their counters, and
/// whether each thread had its own scope back after the commit.
fn together(asks: &[bool], ids: &[&str]) -> Vec<Option<Arc<RequestCost>>> {
    let r = rig();
    let start = Barrier::new(asks.len());
    std::thread::scope(|s| {
        let handles: Vec<_> = asks
            .iter()
            .zip(ids)
            .enumerate()
            .map(|(n, (ask, id))| {
                let (r, start) = (&r, &start);
                s.spawn(move || {
                    let mine = ask.then(RequestCost::new);
                    let _scope = cost::enter(mine.clone());
                    start.wait();
                    let at = u32::try_from(n).expect("small");
                    r.committer
                        .commit(&shard(at), 1, &[rec(id, 1)])
                        .expect("lands");
                    assert_eq!(
                        cost::capture().map(|c| Arc::as_ptr(&c)),
                        mine.as_ref().map(Arc::as_ptr),
                        "the thread's own scope is back"
                    );
                    mine
                })
            })
            .collect();
        let counters: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("ends"))
            .collect();
        assert_eq!(
            r.store.meter().count(S3Op::Put),
            0,
            "the log is the counting store's"
        );
        counters
    })
}

// AC-3, AC-5
#[test]
fn two_writers_sharing_a_window_each_report_half_its_put() {
    // Whichever of the two drives the window, its own request is charged
    // half, not the whole blob.
    let counters = together(&[true, true], &["aa", "bb"]);
    let [a, b] = [0, 1].map(|n| counters[n].clone().expect("asked"));
    assert_eq!(a.milli_requests(S3Op::Put), 500);
    assert_eq!(b.milli_requests(S3Op::Put), 500);
    assert_eq!(a.bytes_written(), b.bytes_written());
    assert!(a.bytes_written() > 0);
}

// AC-4
#[test]
fn a_writer_that_did_not_ask_leaves_its_share_unreported() {
    for asks in [[true, false], [false, true]] {
        let counters = together(&asks, &["aa", "bb"]);
        let asked: Vec<_> = counters.iter().flatten().collect();
        assert_eq!(asked.len(), 1);
        assert_eq!(
            asked[0].milli_requests(S3Op::Put),
            500,
            "its share of the window's bytes, not the whole PUT ({asks:?})"
        );
    }
}

// AC-5
#[test]
fn a_writer_with_a_window_to_itself_reports_the_whole_put() {
    let counters = together(&[true], &["aa"]);
    let only = counters[0].clone().expect("asked");
    assert_eq!(only.milli_requests(S3Op::Put), 1_000);
}

// AC-6
#[test]
fn the_attempts_of_a_hedged_put_are_shared_out_like_a_single_one() {
    let inner = Arc::new(Emulator::new(1 << 20, 3));
    let mut first = true;
    let slow_first = std::sync::Mutex::new(move || std::mem::take(&mut first));
    let hooked = Hooked::store(&inner, move |inner, key, body, buf| {
        if (slow_first.lock().expect("unpoisoned"))() {
            std::thread::sleep(Duration::from_millis(300));
        }
        inner.create_if_absent(key, body, buf)
    });
    let counting = Arc::new(UsageStore::new(
        Shared::new(hooked),
        Arc::new(Ledger::new(8)),
        Arc::new(NoIndex),
    ));
    let log = NodeLogStore::new(counting as Arc<dyn BlobStore>, "", "node-1").expect("names");
    let (a, b) = (RequestCost::new(), RequestCost::new());
    let _scope = cost::enter_scope(Some(Scope::split(
        vec![(Arc::clone(&a), 1), (Arc::clone(&b), 1)],
        2,
    )));
    let blob: Arc<[u8]> = Arc::from(&b"a window's blob"[..]);
    let outcome = log.append_hedged(1, 0, &blob, Duration::from_millis(50), Some(&ThreadPerPut));
    assert_eq!(outcome, Ok(HedgeOutcome::Won));
    // Both attempts, the slow one counted when it answers, half each.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while a.milli_requests(S3Op::Put) < 1_000 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(a.milli_requests(S3Op::Put), 1_000);
    assert_eq!(b.milli_requests(S3Op::Put), 1_000);
}
