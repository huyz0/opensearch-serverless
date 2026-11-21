//! The running bucket total of a reduce.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::BUCKET_LIMIT;
use super::histogram::{HistogramResult, HistogramSpec};
use super::running;
use crate::aggs::partial::{Value, reduce_total};

fn gap_histogram() -> Value {
    // Two buckets a million intervals apart: filling the gap passes the limit.
    let spec = HistogramSpec {
        interval: 1.0,
        ..HistogramSpec::default()
    };
    Value::Histogram(Box::new(HistogramResult {
        spec,
        buckets: vec![(0.0, 1), (1_000_000.0, 1)],
        subs: Default::default(),
    }))
}

#[test]
fn outside_a_scope_the_whole_limit_remains_and_nothing_is_counted() {
    assert_eq!(running::remaining(), BUCKET_LIMIT);
    assert!(running::add(BUCKET_LIMIT + 1));
    assert_eq!(running::remaining(), BUCKET_LIMIT);
}

#[test]
fn a_scope_counts_what_is_added_and_remembers_the_excess() {
    let ((), over) = running::scoped(|| {
        assert!(running::add(BUCKET_LIMIT - 5));
        assert_eq!(running::remaining(), 5);
        assert!(running::add(5));
        assert_eq!(running::remaining(), 0);
        assert!(!running::add(1));
    });
    assert_eq!(over, Some(BUCKET_LIMIT + 1));
    let ((), none) = running::scoped(|| assert!(running::add(BUCKET_LIMIT)));
    assert_eq!(none, None);
    assert_eq!(running::remaining(), BUCKET_LIMIT);
}

#[test]
fn a_scope_inside_a_scope_keeps_the_outer_total() {
    let ((), over) = running::scoped(|| {
        running::add(10);
        let ((), inner) = running::scoped(|| {
            running::add(20);
        });
        assert_eq!(inner, None);
        assert_eq!(running::remaining(), BUCKET_LIMIT - 30);
    });
    assert_eq!(over, None);
}

#[test]
fn a_reduce_total_reports_a_fill_past_the_limit_not_a_mismatch() {
    let err = reduce_total(&[gap_histogram()]).expect_err("too many buckets");
    assert!(err > BUCKET_LIMIT);
}

#[test]
fn a_reduce_total_of_nothing_is_none() {
    assert_eq!(reduce_total(&[]), Ok(None));
}
