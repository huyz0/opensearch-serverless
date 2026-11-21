//! `filters` and `filter` as sub-aggregations of a bucket, run on a real
//! index.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::nested_tests::{first, range_of, run, shard, written};
use crate::aggs::partial::Value;

fn counts_of(value: &Value) -> Vec<(String, u64)> {
    match value {
        Value::Filters(f) => f.buckets.clone(),
        other => panic!("not a filters: {other:?}"),
    }
}

// AC-1
#[test]
fn a_filters_under_a_range_counts_each_buckets_documents() {
    let shard = shard("nested-filters-under-range");
    let outer = first(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100},{"from":100}]},
            "aggs":{"f":{"filters":{"other_bucket":true,"filters":{
                "low":{"range":{"n":{"lt":50}}},"mid":{"range":{"n":{"lt":150}}}}}}}}}}"#,
    );
    let inner: Vec<Vec<(String, u64)>> = range_of(&outer)
        .subs
        .iter()
        .map(|list| counts_of(&list[0]))
        .collect();
    let want = |low: u64, mid: u64, other: u64| {
        vec![
            ("low".to_owned(), low),
            ("mid".to_owned(), mid),
            ("_other_".to_owned(), other),
        ]
    };
    assert_eq!(inner, [want(50, 100, 0), want(0, 50, 50)]);
}

#[test]
fn a_filter_under_a_range_counts_each_buckets_documents() {
    let shard = shard("nested-filter-under-range");
    let outer = first(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100},{"from":100}]},
            "aggs":{"f":{"filter":{"range":{"n":{"lt":50}}}}}}}}"#,
    );
    let inner: Vec<Vec<(String, u64)>> = range_of(&outer)
        .subs
        .iter()
        .map(|list| counts_of(&list[0]))
        .collect();
    assert_eq!(
        inner,
        [vec![("0".to_owned(), 50)], vec![("0".to_owned(), 0)]]
    );
}

#[test]
fn two_nested_filters_each_read_their_own_queries() {
    let shard = shard("nested-filters-two");
    let outer = first(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100}]},
            "aggs":{"a":{"filter":{"range":{"n":{"lt":10}}}},
                    "b":{"filters":{"filters":[{"range":{"n":{"lt":20}}},{"range":{"n":{"lt":30}}}]}},
                    "c":{"filter":{"range":{"n":{"lt":40}}}}}}}}"#,
    );
    let list = &range_of(&outer).subs[0];
    let counts: Vec<Vec<u64>> = list
        .iter()
        .map(|v| counts_of(v).into_iter().map(|(_, n)| n).collect())
        .collect();
    assert_eq!(counts, [vec![10], vec![20, 30], vec![40]]);
}

#[test]
fn a_filter_under_a_range_under_a_range_reads_its_own_query() {
    let shard = shard("nested-filter-deep");
    let outer = first(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100}]},
            "aggs":{"i":{"range":{"field":"n","ranges":[{"to":50},{"from":50}]},
                "aggs":{"f":{"filter":{"range":{"n":{"lt":25}}}}}}}}}}"#,
    );
    let inner = range_of(&range_of(&outer).subs[0][0]);
    let counts: Vec<u64> = inner
        .subs
        .iter()
        .map(|list| counts_of(&list[0])[0].1)
        .collect();
    assert_eq!(counts, [25, 0]);
}

#[test]
fn a_nested_filters_is_written_under_its_bucket() {
    let shard = shard("nested-filters-written");
    let parts = run(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","keyed":true,"ranges":[{"key":"lo","to":100}]},
            "aggs":{"f":{"filter":{"range":{"n":{"lt":50}}}}}}}}"#,
    )
    .expect("searches");
    assert_eq!(
        written(&parts),
        r#","aggregations":{"o":{"buckets":{"lo":{"to":100.0,"doc_count":100,"f":{"doc_count":50}}}}}"#
    );
}

// AC-4
#[test]
fn a_filter_under_a_keyword_terms_is_still_refused() {
    let shard = shard("nested-filter-keyword");
    let err = run(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"f":{"filter":{"match_all":{}}}}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(err.status, 400);
}

#[test]
fn an_unmapped_parents_filters_do_not_shift_its_siblings_queries() {
    let shard = shard("nested-filter-unmapped");
    let outer = first(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100}]},
            "aggs":{"u":{"range":{"field":"missing","ranges":[{"to":1}]},
                        "aggs":{"x":{"filter":{"range":{"n":{"lt":5}}}}}},
                    "f":{"filter":{"range":{"n":{"lt":30}}}}}}}}"#,
    );
    let list = &range_of(&outer).subs[0];
    assert_eq!(counts_of(&list[1]), [("0".to_owned(), 30)]);
}
