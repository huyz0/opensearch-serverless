//! docs/specs/reader-tail-fresh: a reader answers for a shard only while it
//! knows it holds every write its writer answered as refreshed.

#![cfg(feature = "reader")]

use std::cell::Cell;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use oss_node::overlay_builder::TailFetcher;
use oss_node::tail_fresh::TailFresh;
use oss_node::tail_held::TailHeld;
use oss_transport::actions::tail::{CHECK, FetchRequest, FetchResponse, NEW, NONE};

const U: &str = "uuid-a";
const W: &str = "writer";

/// Answers each fetch with the next scripted answer; `None` is a writer that
/// does not answer.
#[derive(Default)]
struct Fetcher {
    answers: Mutex<Vec<Option<FetchResponse>>>,
    asked: Mutex<Vec<(i64, i64)>>,
}

impl TailFetcher for Fetcher {
    fn fetch(&self, _: &str, request: &FetchRequest) -> Option<FetchResponse> {
        self.asked
            .lock()
            .unwrap()
            .push((request.subscription, request.after_seq_no));
        let mut answers = self.answers.lock().unwrap();
        if answers.is_empty() {
            None
        } else {
            answers.remove(0)
        }
    }
}

fn answer(subscription: i64, complete: bool, claimed: i64) -> Option<FetchResponse> {
    Some(FetchResponse {
        records: Vec::new(),
        subscription,
        complete,
        claimed_through: claimed,
    })
}

struct Rig {
    held: Arc<TailHeld>,
    fetcher: Arc<Fetcher>,
    now: Arc<AtomicI64>,
    fresh: TailFresh,
}

fn rig(grants: bool) -> Rig {
    let held = Arc::new(TailHeld::new());
    let fetcher = Arc::new(Fetcher::default());
    let now = Arc::new(AtomicI64::new(1_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst)) as oss_node::tail_reader_link::Clock
    };
    let fresh = TailFresh::new(
        held.clone(),
        fetcher.clone(),
        "me".to_owned(),
        clock,
        grants,
    );
    Rig {
        held,
        fetcher,
        now,
        fresh,
    }
}

fn ensure(r: &Rig, commit: i64) -> (bool, u32) {
    let applied = Cell::new(0_u32);
    let fresh = r.fresh.ensure_fresh("logs", U, 0, W, commit, &|| {
        applied.set(applied.get().saturating_add(1))
    });
    (fresh, applied.get())
}

#[test]
fn without_grants_every_search_may_answer_and_nothing_is_asked() {
    let r = rig(false);
    assert_eq!(ensure(&r, 5), (true, 0));
    assert!(r.fetcher.asked.lock().unwrap().is_empty());
}

#[test]
fn a_serving_reader_takes_what_is_held_before_it_answers_and_asks_nothing() {
    let r = rig(false);
    // Without the setting a search does not even apply what is held.
    assert_eq!(ensure(&r, 5), (true, 0));
    let held = Arc::new(TailHeld::new());
    let serving = TailFresh::new(
        held,
        r.fetcher.clone(),
        "me".to_owned(),
        Arc::new(|| 1_000_i64) as oss_node::tail_reader_link::Clock,
        false,
    )
    .applying(true);
    let applied = Cell::new(0_u32);
    assert!(serving.ensure_fresh("logs", U, 0, W, 5, &|| applied.set(applied.get() + 1)));
    assert_eq!(applied.get(), 1);
    assert!(r.fetcher.asked.lock().unwrap().is_empty());
}

#[test]
fn under_a_grant_a_search_answers_without_asking() {
    let r = rig(true);
    r.held.track(U, 0, 5);
    r.held
        .answered(U, 0, W, NEW, 1_000, &answer(7, true, 9).unwrap());
    let (fresh, applied) = ensure(&r, 5);
    assert!(fresh);
    assert_eq!(applied, 1, "the engine takes what is held first");
    assert!(r.fetcher.asked.lock().unwrap().is_empty());
    assert_eq!(r.fresh.counts(), (0, 0, 0));
}

#[test]
fn without_a_grant_the_writer_is_asked_to_renew_or_to_subscribe_anew() {
    let r = rig(true);
    r.held.track(U, 0, 5);
    // No subscription held: a subscription anew, whose answer grants.
    r.fetcher.answers.lock().unwrap().push(answer(7, true, 9));
    assert!(ensure(&r, 5).0);
    assert_eq!(r.fetcher.asked.lock().unwrap().as_slice(), [(NEW, 5)]);
    assert!(r.held.granted(U, 0, W, 1_000));
    // The grant runs out; the next search renews the subscription it holds.
    r.now.store(1_700, Ordering::SeqCst);
    r.fetcher.answers.lock().unwrap().push(answer(7, true, 9));
    assert!(ensure(&r, 5).0);
    assert_eq!(r.fetcher.asked.lock().unwrap()[1], (7, 5));
    assert!(r.held.granted(U, 0, W, 1_700));
    assert_eq!(r.fresh.counts().0, 2);
}

#[test]
fn a_grant_that_ran_out_while_the_search_took_the_records_still_answers() {
    let r = rig(true);
    r.held.track(U, 0, 5);
    r.fetcher.answers.lock().unwrap().push(answer(7, true, 9));
    // Taking what is held outlasts the grant's span (a loaded node).
    let fresh = r.fresh.ensure_fresh("logs", U, 0, W, 5, &|| {
        r.now.store(2_000, Ordering::SeqCst);
    });
    assert!(fresh);
    assert_eq!(r.fresh.counts(), (1, 0, 0), "asked once, refused none");
}

#[test]
fn a_shard_with_no_engine_only_asks_how_far_refreshing_writes_went() {
    let r = rig(true);
    // Nothing tracks the shard: a CHECK, which grants nothing.
    r.fetcher
        .answers
        .lock()
        .unwrap()
        .push(answer(NONE, false, 3));
    assert!(ensure(&r, 5).0, "the commit holds everything claimed");
    assert_eq!(r.fetcher.asked.lock().unwrap().as_slice(), [(CHECK, 5)]);
    assert!(!r.held.granted(U, 0, W, 1_000));
    // The commit is behind what the writer claimed: refused.
    r.fetcher
        .answers
        .lock()
        .unwrap()
        .push(answer(NONE, false, 9));
    assert!(!ensure(&r, 5).0);
    assert_eq!(r.fresh.counts().1, 1, "one refusal");
}

#[test]
fn a_writer_that_does_not_answer_is_treated_as_dead() {
    let r = rig(true);
    r.held.track(U, 0, 5);
    // Nothing answers the ask: the reader serves its commit.
    assert!(ensure(&r, 5).0);
    assert_eq!(r.fetcher.asked.lock().unwrap().len(), 1, "no second ask");
    assert_eq!(r.fresh.counts(), (1, 0, 1));
}

#[test]
fn a_writer_that_did_not_answer_is_not_asked_again_for_the_same_search() {
    let r = rig(true);
    r.held.track(U, 0, 5);
    // The writer shows gone to the grant ask and alive a moment later (a
    // lease view refreshed between the two): the second ask would claim
    // writes above the commit and refuse a search the first let through.
    r.fetcher.answers.lock().unwrap().push(None);
    r.fetcher
        .answers
        .lock()
        .unwrap()
        .push(answer(NONE, false, 9));
    assert!(ensure(&r, 5).0);
    assert_eq!(r.fetcher.asked.lock().unwrap().len(), 1);
    assert_eq!(r.fresh.counts(), (1, 0, 1), "no refusal");
}

#[test]
fn an_answer_without_a_grant_leaves_the_commit_to_be_compared_with_the_claim() {
    let r = rig(true);
    r.held.track(U, 0, 5);
    // Not complete: no grant; the claim is above the commit, so refused.
    r.fetcher.answers.lock().unwrap().push(answer(7, false, 8));
    assert!(!ensure(&r, 5).0);
    assert_eq!(r.fetcher.asked.lock().unwrap().len(), 1, "no second ask");
    // The claim is at or below the commit: it may answer.
    r.held.track(U, 0, 5);
    r.fetcher.answers.lock().unwrap().push(answer(8, false, 5));
    assert!(ensure(&r, 5).0);
}

#[test]
fn what_to_ask_follows_the_engine_and_the_subscription_held() {
    let held = TailHeld::new();
    // No engine: only the writer's claim is of use.
    assert_eq!(held.subscription_to_ask(U, 0, W), CHECK);
    held.track(U, 0, 5);
    assert_eq!(held.subscription_to_ask(U, 0, W), NEW);
    held.answered(U, 0, W, NEW, 1_000, &answer(7, true, 9).unwrap());
    assert_eq!(held.subscription_to_ask(U, 0, W), 7);
    // Another writer's grant is no grant of this one's.
    assert_eq!(held.subscription_to_ask(U, 0, "other"), NEW);
    // An engine that left a record out cannot use a subscription.
    held.skipped(U, 0);
    assert_eq!(held.subscription_to_ask(U, 0, W), CHECK);
    // Nor can one whose shard refuses for want of room.
    held.track(U, 0, 5);
    assert_eq!(held.subscription_to_ask(U, 0, W), 7);
    let full: Vec<_> = (6..=100_010)
        .map(|s| oss_transport::actions::tail::Record {
            id: format!("d{s}"),
            deletion: false,
            source: "{}".into(),
            seq_no: s,
            primary_term: 1,
            version: 1,
            routing: None,
        })
        .collect();
    assert_eq!(held.take(U, 0, &full, 1_000), None);
    assert_eq!(held.subscription_to_ask(U, 0, W), CHECK);
}

// docs/specs/node-cluster-visibility AC-3
#[test]
fn a_running_check_follows_the_mode_set_last() {
    let r = rig(false);
    assert_eq!(r.fresh.mode(), (false, false));
    assert_eq!(
        ensure(&r, 5),
        (true, 0),
        "owner: nothing applied, nothing asked"
    );
    r.fresh.set_mode(false, true);
    assert_eq!(r.fresh.mode(), (false, true));
    assert_eq!(
        ensure(&r, 5),
        (true, 1),
        "serving: held records taken first"
    );
    assert!(r.fetcher.asked.lock().unwrap().is_empty());
    r.fresh.set_mode(true, true);
    assert_eq!(r.fresh.mode(), (true, true));
    // Everywhere asks the writer (the fake answers nothing: treated as gone).
    let _ = ensure(&r, 5);
    assert!(
        !r.fetcher.asked.lock().unwrap().is_empty(),
        "everywhere asks"
    );
    r.fresh.set_mode(false, false);
    let before = r.fetcher.asked.lock().unwrap().len();
    assert_eq!(ensure(&r, 5), (true, 0), "back to owner");
    assert_eq!(r.fetcher.asked.lock().unwrap().len(), before);
}
