//! The error bounds of a `terms`: a shard's, a bucket's and the reduce's, and
//! an empty shard's result.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::terms::{
    TermsOrder, TermsResult, TermsSpec, bucket_errors, drop_below, reduce_results, shard_error,
};
use super::terms_collect::empty;

fn spec(order: TermsOrder) -> TermsSpec {
    TermsSpec {
        size: 2,
        shard_size: 2,
        order,
        ..TermsSpec::default()
    }
}

fn part(order: TermsOrder, buckets: &[(i64, u64)]) -> TermsResult {
    TermsResult {
        spec: spec(order),
        buckets: buckets.to_vec(),
        ..TermsResult::default()
    }
}

#[test]
fn a_full_count_descending_shard_reports_its_own_error_or_its_smallest_count() {
    let s = spec(TermsOrder::CountDesc);
    let buckets = [(1, 10), (2, 4)];
    assert_eq!(shard_error(&s, &buckets, 5), 5);
    assert_eq!(shard_error(&s, &buckets, 0), 4);
    assert_eq!(shard_error(&s, &buckets, -3), 4);
    // Not full, empty, or ordered by key: nothing was cut.
    assert_eq!(shard_error(&s, &buckets[..1], 5), 0);
    assert_eq!(shard_error(&s, &[] as &[(i64, u64)], 5), 0);
    assert_eq!(shard_error(&spec(TermsOrder::KeyAsc), &buckets, 5), 0);
    assert_eq!(shard_error(&spec(TermsOrder::CountAsc), &buckets, 5), -1);
}

#[test]
fn bucket_errors_are_empty_unless_asked_for() {
    assert!(bucket_errors(false, 5, &[(vec![1], &[4][..], 0)], &[1]).is_empty());
}

#[test]
fn a_bucket_error_adds_the_stored_ones_less_each_parts_own() {
    let parts = [(vec![1, 2], &[4, 2][..], 1), (vec![1], &[3][..], 2)];
    // Key 1: (4 - 1) + (3 - 2) = 4; key 2: 2 - 1 = 1. The sum error, 6, is
    // added to each.
    assert_eq!(bucket_errors(true, 6, &parts, &[1, 2]), [10, 7]);
    // A key no part holds gets the sum alone.
    assert_eq!(bucket_errors(true, 6, &parts, &[9]), [6]);
}

#[test]
fn an_unbounded_stored_error_makes_the_bucket_unbounded_whatever_came_before_or_after() {
    let before = [(vec![1], &[4][..], 0), (vec![1], &[-1][..], 0)];
    assert_eq!(bucket_errors(true, 5, &before, &[1]), [-1]);
    let after = [(vec![1], &[-1][..], 0), (vec![1], &[3][..], 0)];
    assert_eq!(bucket_errors(true, 5, &after, &[1]), [-1]);
}

#[test]
fn an_unbounded_sum_makes_every_bucket_unbounded() {
    let parts = [(vec![1], &[4][..], 0)];
    assert_eq!(bucket_errors(true, -1, &parts, &[1, 7]), [-1, -1]);
    assert_eq!(bucket_errors(true, 1, &parts, &[1, 7]), [5, 1]);
}

#[test]
fn an_unbounded_part_keeps_the_reduced_error_unbounded() {
    // The first part is cut by an ascending count (unbounded); the second
    // is full and bounded by its smallest count, 4.
    let a = part(TermsOrder::CountAsc, &[(1, 1), (2, 2)]);
    let b = part(TermsOrder::CountDesc, &[(3, 9), (4, 4)]);
    assert_eq!(reduce_results(&[&a, &b]).doc_count_error, -1);
    // Bounded parts add up.
    let c = part(TermsOrder::CountDesc, &[(1, 8), (2, 3)]);
    assert_eq!(reduce_results(&[&b, &c]).doc_count_error, 7);
}

#[test]
fn an_empty_shard_result_keeps_the_spec_and_has_no_buckets() {
    let s = TermsSpec {
        size: 3,
        shard_size: 7,
        ..TermsSpec::default()
    };
    let r = empty(&s);
    assert_eq!(r.spec, s);
    assert!(r.buckets.is_empty());
    assert_eq!((r.other_doc_count, r.doc_count_error), (0, 0));
}

#[test]
fn a_reduce_sums_the_bounded_parts_whatever_their_order_and_size() {
    let full = |order, low| part(order, &[(1, 9), (2, low)]);
    let unbounded = full(TermsOrder::CountAsc, 1);
    let bounded = full(TermsOrder::CountDesc, 4);
    let one = full(TermsOrder::CountDesc, 1);
    // A bounded part first: the unbounded one after it still wins.
    assert_eq!(reduce_results(&[&bounded, &unbounded]).doc_count_error, -1);
    // A bound of 1 is a bound, not "unbounded".
    assert_eq!(reduce_results(&[&bounded, &one]).doc_count_error, 5);
    assert_eq!(reduce_results(&[&one, &bounded]).doc_count_error, 5);
    // A single part: unbounded stays so, bounded is exact.
    assert_eq!(reduce_results(&[&unbounded]).doc_count_error, -1);
    assert_eq!(reduce_results(&[&one]).doc_count_error, 0);
    assert_eq!(reduce_results(&[&bounded]).doc_count_error, 0);
}

#[test]
fn a_shard_minimum_drops_exactly_the_buckets_under_it() {
    let run = |min, buckets: &[(i64, u64)]| {
        let mut b = buckets.to_vec();
        let mut subs: Vec<u8> = (0..b.len() as u8).collect();
        let dropped = drop_below(min, &mut b, &mut subs);
        (dropped, b, subs)
    };
    // Mixed: the ones under 4 go, the one equal to it stays.
    assert_eq!(
        run(4, &[(1, 5), (2, 1), (3, 2), (4, 4)]),
        (3, vec![(1, 5), (4, 4)], vec![0, 3])
    );
    // Every bucket under it: all go.
    assert_eq!(run(5, &[(1, 1), (2, 2)]), (3, vec![], vec![]));
    // None under it, or no minimum: untouched.
    assert_eq!(
        run(2, &[(1, 5), (2, 2)]),
        (0, vec![(1, 5), (2, 2)], vec![0, 1])
    );
    assert_eq!(run(0, &[(1, 5)]), (0, vec![(1, 5)], vec![0]));
}
