//! docs/specs/reader-tail-held: the reader holds the writes its writer
//! pushed.

#![cfg(feature = "reader")]

use oss_node::tail_held::{GRANT_MILLIS, KEEP_AFTER_CLOSE_MILLIS, MAX_RECORDS, TailHeld};
use oss_transport::actions::tail::{FetchResponse, NONE, PushRequest, Record, ShardRef};

const U: &str = "uuid-a";

fn rec(seq: i64) -> Record {
    Record {
        id: format!("d{seq}"),
        source: "{}".into(),
        seq_no: seq,
        primary_term: 1,
        version: 1,
        ..Record::default()
    }
}

fn recs(range: std::ops::RangeInclusive<i64>) -> Vec<Record> {
    range.map(rec).collect()
}

fn ids(held: &[(u64, Record)]) -> Vec<u64> {
    held.iter().map(|(o, _)| *o).collect()
}

fn answer(subscription: i64, complete: bool, records: Vec<Record>) -> FetchResponse {
    FetchResponse {
        records,
        subscription,
        complete,
        claimed_through: 0,
    }
}

#[test]
fn an_untracked_shard_is_unwanted_and_a_tracked_one_keeps_what_is_above_its_checkpoint() {
    let h = TailHeld::new();
    assert_eq!(h.take(U, 0, &recs(1..=2), 0), None);
    h.track(U, 0, 5);
    assert_eq!(h.take(U, 0, &recs(4..=8), 0), Some(3), "6, 7, 8");
    assert_eq!(ids(&h.from(U, 0, 0)), [0, 1, 2]);
    assert_eq!(h.next_ordinal(U, 0), 3);
    assert_eq!(h.take(U, 0, &[], 0), Some(0));
    assert_eq!(h.take(U, 1, &recs(1..=1), 0), None, "another shard");
    assert_eq!(h.take("other", 0, &recs(1..=1), 0), None, "another uuid");
}

#[test]
fn a_full_shard_refuses_until_an_engine_opens_on_a_newer_commit() {
    assert_eq!(MAX_RECORDS, 100_000);
    let h = TailHeld::new();
    h.track(U, 0, 0);
    let n = MAX_RECORDS as i64;
    assert_eq!(h.take(U, 0, &recs(1..=n), 0), Some(MAX_RECORDS));
    assert_eq!(h.take(U, 0, &recs(n + 1..=n + 1), 0), None, "full");
    assert_eq!(h.take(U, 0, &recs(1..=1), 0), None, "and refusing");
    h.track(U, 0, 10);
    assert_eq!(
        h.from(U, 0, 0).len(),
        MAX_RECORDS - 10,
        "dropped through 10"
    );
    assert_eq!(h.next_ordinal(U, 0), n as u64, "ordinals go on");
    assert_eq!(h.take(U, 0, &recs(n + 1..=n + 1), 0), Some(1), "room again");
    assert_eq!(h.from(U, 0, n as u64).len(), 1);
}

#[test]
fn a_shard_is_kept_thirty_seconds_after_its_last_engine_closes() {
    assert_eq!(KEEP_AFTER_CLOSE_MILLIS, 30_000);
    let h = TailHeld::new();
    h.track(U, 0, 0);
    h.track(U, 0, 0);
    h.untrack(U, 0, 1_000);
    assert_eq!(
        h.take(U, 0, &recs(1..=1), 90_000),
        Some(1),
        "one engine is left"
    );
    h.untrack(U, 0, 2_000);
    assert_eq!(
        h.take(U, 0, &recs(2..=2), 32_000),
        Some(1),
        "30 s is not over"
    );
    assert_eq!(h.take(U, 0, &recs(3..=3), 32_001), None, "past it");
    assert_eq!(h.take(U, 0, &recs(4..=4), 32_002), None, "and gone");
    assert_eq!(h.next_ordinal(U, 0), 0);
    // An expired shard is let go on the next untrack of any shard.
    h.track(U, 1, 0);
    h.untrack(U, 1, 3_000);
    h.track(U, 2, 0);
    h.untrack(U, 2, 40_000);
    assert_eq!(h.take(U, 1, &recs(1..=1), 40_001), None, "(U, 1) expired");
    assert_eq!(h.take(U, 2, &recs(1..=1), 40_001), Some(1));
    assert_eq!(h.held_shards(), 1, "(U, 1) was let go by the sweep");
    h.untrack(U, 9, 41_000);
    assert_eq!(h.held_shards(), 1);
}

#[test]
fn from_gives_what_is_held_at_or_after_an_ordinal_in_order() {
    let h = TailHeld::new();
    assert!(h.from(U, 0, 0).is_empty());
    assert_eq!(h.next_ordinal(U, 0), 0);
    h.track(U, 0, 0);
    h.take(U, 0, &recs(1..=4), 0);
    let held = h.from(U, 0, 2);
    assert_eq!(ids(&held), [2, 3]);
    assert_eq!(held[0].1.seq_no, 3);
    assert!(h.from(U, 0, 4).is_empty());
}

#[test]
fn an_answer_sets_a_renews_or_clears_the_grant() {
    let h = TailHeld::new();
    h.track(U, 0, 0);
    // No subscription clears only the granting writer's grant.
    h.answered(U, 0, "w1", NONE, 1_000, &answer(3, true, vec![]));
    assert!(h.granted(U, 0, "w1", 1_499));
    h.answered(U, 0, "w2", NONE, 1_100, &answer(NONE, false, vec![]));
    assert!(
        h.granted(U, 0, "w1", 1_499),
        "another writer's none changes nothing"
    );
    // The grant runs from the send, for 500 ms.
    assert_eq!(GRANT_MILLIS, 500);
    assert!(h.granted(U, 0, "w1", 1_499));
    assert!(!h.granted(U, 0, "w1", 1_500));
    // A renewal extends and never shortens.
    h.answered(U, 0, "w1", 3, 1_300, &answer(3, false, vec![]));
    assert!(h.granted(U, 0, "w1", 1_799), "extended; complete kept");
    h.answered(U, 0, "w1", 3, 1_000, &answer(3, false, vec![]));
    assert!(h.granted(U, 0, "w1", 1_799), "never shortened");
    // A new subscription replaces it, complete only if the answer was.
    h.answered(U, 0, "w1", -1, 2_000, &answer(4, false, vec![]));
    assert!(
        !h.granted(U, 0, "w1", 2_100),
        "an incomplete answer grants nothing"
    );
    h.answered(U, 0, "w1", -1, 2_000, &answer(5, true, recs(1..=2)));
    assert!(h.granted(U, 0, "w1", 2_100));
    assert_eq!(h.from(U, 0, 0).len(), 2, "its records were taken");
    h.answered(U, 0, "w1", NONE, 2_200, &answer(NONE, false, vec![]));
    assert!(!h.granted(U, 0, "w1", 2_201), "the writer's none clears it");
}

#[test]
fn an_answer_whose_records_were_refused_is_not_complete() {
    let h = TailHeld::new();
    h.track(U, 0, 0);
    let n = MAX_RECORDS as i64;
    h.take(U, 0, &recs(1..=n), 0);
    h.answered(U, 0, "w1", -1, 0, &answer(1, true, recs(n + 1..=n + 1)));
    assert!(!h.granted(U, 0, "w1", 1));
}

#[test]
fn a_grant_needs_every_condition() {
    let h = TailHeld::new();
    assert!(!h.granted(U, 0, "w1", 0), "nothing held");
    h.track(U, 0, 0);
    h.answered(U, 0, "w1", -1, 0, &answer(1, true, vec![]));
    assert!(h.granted(U, 0, "w1", 100));
    assert!(!h.granted(U, 0, "w2", 100), "another writer");
    assert!(!h.granted(U, 1, "w1", 100), "another shard");
    h.skipped(U, 0);
    h.skipped(U, 7);
    assert!(!h.granted(U, 0, "w1", 100), "skipped");
    h.track(U, 0, 0);
    h.answered(U, 0, "w1", -1, 0, &answer(2, true, vec![]));
    assert!(h.granted(U, 0, "w1", 100), "a new engine clears skipped");
    h.untrack(U, 0, 50);
    h.untrack(U, 0, 50);
    assert!(!h.granted(U, 0, "w1", 100), "no engine");
    // An answer with no subscription never grants.
    h.track(U, 0, 0);
    h.answered(U, 0, "w1", -1, 0, &answer(-1, true, vec![]));
    assert!(!h.granted(U, 0, "w1", 100));
}

#[test]
fn handle_answers_the_shards_whose_records_were_not_taken() {
    let h = TailHeld::new();
    h.track(U, 0, 0);
    let shard = |s: i32| ShardRef {
        index_name: "logs".into(),
        index_uuid: U.into(),
        shard: s,
    };
    let request = PushRequest {
        shards: vec![
            (shard(0), recs(1..=2)),
            (shard(1), recs(1..=1)),
            (shard(2), vec![]),
        ],
        ..PushRequest::default()
    };
    let answer = h.handle(&request, 0);
    assert_eq!(answer.unwanted, [shard(1), shard(2)]);
    assert_eq!(h.from(U, 0, 0).len(), 2);
}

#[test]
fn an_expired_shard_that_was_refusing_is_still_let_go() {
    let h = TailHeld::new();
    h.track(U, 0, 0);
    let n = MAX_RECORDS as i64;
    h.take(U, 0, &recs(1..=n + 1), 0);
    h.untrack(U, 0, 10);
    assert_eq!(h.held_shards(), 1, "kept for its keep");
    assert_eq!(h.take(U, 0, &recs(1..=1), 30_011), None);
    assert_eq!(h.held_shards(), 0);
}

#[test]
fn subscription_zero_renews_and_a_lower_checkpoint_never_lowers_the_held_one() {
    let h = TailHeld::new();
    h.track(U, 0, 0);
    h.answered(U, 0, "w1", -1, 100, &answer(0, true, vec![]));
    h.answered(U, 0, "w1", 0, 200, &answer(0, false, vec![]));
    assert!(
        h.granted(U, 0, "w1", 650),
        "0 is a subscription: complete is kept"
    );
    h.track(U, 0, 10);
    h.track(U, 0, 3);
    assert_eq!(
        h.take(U, 0, &recs(5..=5), 0),
        Some(0),
        "5 is below the held 10"
    );
    assert_eq!(h.take(U, 0, &recs(11..=11), 0), Some(1));
}
