//! docs/specs/writer-tail-keep: a writer keeps acknowledged writes until a
//! commit holds them.

#![cfg(feature = "writer")]

use oss_format::WalRecord;
use oss_node::tail_keep::{Limits, TailKeep};

const U: &str = "uuid-a";

fn rec(seq: i64, source: &str) -> WalRecord<'static> {
    WalRecord::index(format!("d{seq}"), source.to_owned(), seq, 1, 1)
}

fn seqs(records: &[WalRecord<'static>]) -> Vec<i64> {
    records.iter().map(WalRecord::seq_no).collect()
}

fn small(records: usize, shard_bytes: u64, node_bytes: u64) -> TailKeep {
    TailKeep::with_limits(Limits {
        shard_records: records,
        shard_bytes,
        node_bytes,
    })
}

#[test]
fn acked_records_come_back_oldest_first_and_only_those_above() {
    let keep = TailKeep::new();
    keep.acked(U, 0, &[rec(4, "{}"), rec(5, "{}"), rec(6, "{}")], true);
    assert_eq!(seqs(&keep.kept_after(U, 0, -1)), [4, 5, 6]);
    assert_eq!(seqs(&keep.kept_after(U, 0, 4)), [5, 6]);
    assert!(keep.kept_after(U, 0, 6).is_empty());
    assert!(keep.keeps(U, 0));
    assert!(!keep.keeps(U, 1), "another shard");
    assert!(keep.kept_after("other", 0, -1).is_empty(), "another uuid");
}

#[test]
fn a_publish_drops_what_it_holds_and_a_later_ack_below_it_is_not_kept() {
    let keep = TailKeep::new();
    keep.acked(U, 0, &[rec(1, "abc"), rec(2, "abc"), rec(3, "abc")], true);
    assert_eq!(keep.kept_bytes(), 3 * (2 + 3));
    keep.published(U, 0, 2);
    assert_eq!(seqs(&keep.kept_after(U, 0, -1)), [3]);
    assert_eq!(keep.kept_bytes(), 2 + 3);
    keep.acked(U, 0, &[rec(2, "abc"), rec(4, "abc")], true);
    assert_eq!(seqs(&keep.kept_after(U, 0, -1)), [3, 4], "2 is published");
    keep.published(U, 0, 1);
    assert_eq!(seqs(&keep.kept_after(U, 0, -1)), [3, 4], "never backwards");
    keep.published(U, 0, 4);
    assert_eq!(keep.kept_bytes(), 0);
    keep.acked(U, 0, &[rec(3, "x")], true);
    assert!(keep.kept_after(U, 0, -1).is_empty());
}

#[test]
fn a_reader_below_what_was_published_is_told_of_a_gap() {
    let keep = TailKeep::new();
    keep.acked(
        U,
        0,
        &(1..=5).map(|s| rec(s, "{}")).collect::<Vec<_>>(),
        true,
    );
    assert!(keep.complete_after(U, 0, 0));
    keep.published(U, 0, 4);
    assert_eq!(seqs(&keep.kept_after(U, 0, 2)), [5], "3 and 4 are gone");
    assert!(!keep.complete_after(U, 0, 2));
    assert!(!keep.complete_after(U, 0, 3));
    assert!(keep.complete_after(U, 0, 4));
}

#[test]
fn the_floor_is_the_lowest_first_ack_minus_one() {
    let keep = TailKeep::new();
    assert!(!keep.complete_after(U, 0, 100), "never acked");
    keep.acked(U, 0, &[rec(9, "{}"), rec(7, "{}"), rec(8, "{}")], true);
    assert!(!keep.complete_after(U, 0, 5), "below the floor of 6");
    assert!(keep.complete_after(U, 0, 6), "at the floor");
    assert!(keep.complete_after(U, 0, 8));
    keep.acked(U, 0, &[rec(1, "{}")], true);
    assert!(!keep.complete_after(U, 0, 5), "the floor is set once");
}

#[test]
fn past_the_shard_bounds_the_oldest_go_and_the_reader_is_told_of_a_gap() {
    let keep = small(3, 1 << 20, 1 << 30);
    keep.acked(
        U,
        0,
        &(1..=5).map(|s| rec(s, "{}")).collect::<Vec<_>>(),
        true,
    );
    assert_eq!(seqs(&keep.kept_after(U, 0, -1)), [3, 4, 5]);
    assert!(!keep.complete_after(U, 0, 1));
    assert!(
        keep.complete_after(U, 0, 2),
        "a commit at 2 holds what was dropped"
    );

    // 4 records of 2 + 6 bytes; the shard holds 20 bytes: two fit.
    let keep = small(100, 20, 1 << 30);
    keep.acked(
        U,
        0,
        &(1..=4).map(|s| rec(s, "123456")).collect::<Vec<_>>(),
        true,
    );
    assert_eq!(seqs(&keep.kept_after(U, 0, -1)), [3, 4]);
    assert_eq!(keep.kept_bytes(), 16);
    assert!(!keep.complete_after(U, 0, 1));
    assert!(keep.complete_after(U, 0, 2));
}

#[test]
fn the_exact_shard_bounds_keep_everything() {
    let keep = small(3, 24, 1 << 30);
    keep.acked(
        U,
        0,
        &(1..=3).map(|s| rec(s, "123456")).collect::<Vec<_>>(),
        true,
    );
    assert_eq!(
        seqs(&keep.kept_after(U, 0, -1)),
        [1, 2, 3],
        "3 records, 24 bytes"
    );
    assert!(keep.complete_after(U, 0, 0));
}

#[test]
fn past_the_node_bound_a_record_is_not_kept_and_bytes_return_on_release() {
    let keep = small(100, 1 << 20, 15);
    keep.acked(
        U,
        0,
        &[rec(1, "123456"), rec(2, "123456"), rec(3, "123456")],
        true,
    );
    // Refused once the node holds more than the bound: 8 is not over 15, 16 is.
    assert_eq!(seqs(&keep.kept_after(U, 0, -1)), [1, 2]);
    assert_eq!(keep.kept_bytes(), 16);
    assert!(!keep.complete_after(U, 0, 2), "3 was not kept");
    assert!(keep.complete_after(U, 0, 3));
    keep.acked(U, 1, &[rec(1, "1")], true);
    assert!(keep.kept_after(U, 1, -1).is_empty(), "the node is full");
    keep.published(U, 0, 2);
    assert_eq!(keep.kept_bytes(), 0);
    keep.acked(U, 1, &[rec(2, "1")], true);
    assert_eq!(seqs(&keep.kept_after(U, 1, -1)), [2]);
    keep.released(U, 1);
    assert_eq!(keep.kept_bytes(), 0);
}

#[test]
fn exactly_at_the_node_bound_is_not_over_it() {
    let keep = small(100, 1 << 20, 16);
    keep.acked(
        U,
        0,
        &[rec(1, "123456"), rec(2, "123456"), rec(3, "123456")],
        true,
    );
    assert_eq!(seqs(&keep.kept_after(U, 0, -1)), [1, 2, 3]);
}

#[test]
fn a_write_not_to_be_pushed_begins_the_keep_and_stores_nothing() {
    let keep = TailKeep::new();
    keep.acked(U, 0, &[rec(10, "{}"), rec(11, "{}")], false);
    assert!(keep.keeps(U, 0));
    assert!(keep.kept_after(U, 0, -1).is_empty());
    assert_eq!(keep.kept_bytes(), 0);
    assert!(!keep.complete_after(U, 0, 8));
    assert!(keep.complete_after(U, 0, 9));
    keep.acked(U, 0, &[], true);
    keep.acked(U, 2, &[], false);
    assert!(!keep.keeps(U, 2), "no records begin nothing");
}

#[test]
fn a_released_shard_keeps_nothing_and_a_new_ack_begins_a_new_floor() {
    let keep = TailKeep::new();
    keep.acked(U, 0, &[rec(3, "{}")], true);
    keep.released(U, 0);
    keep.released(U, 7);
    assert!(!keep.keeps(U, 0));
    assert!(keep.kept_after(U, 0, -1).is_empty());
    assert!(!keep.complete_after(U, 0, 0));
    assert_eq!(keep.kept_bytes(), 0);
    keep.acked(U, 0, &[rec(20, "{}")], true);
    assert!(!keep.complete_after(U, 0, 18));
    assert!(keep.complete_after(U, 0, 19));
}

#[test]
fn the_default_limits_are_the_shells() {
    let l = Limits::default();
    assert_eq!(l.shard_records, 20_000);
    assert_eq!(l.shard_bytes, 16 << 20);
    assert_eq!(l.node_bytes, 256 << 20);
}
