//! `filters` parsed, reduced and written out.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::filters::{FiltersResult, FiltersSpec, reduce};
use super::filters_render::write_filters;
use crate::aggs::partial::Value;
use crate::dsl::{Query, parse_chunks};

fn spec_of(filters_body: &str) -> FiltersSpec {
    let body = format!(r#"{{"aggs":{{"f":{{"filters":{filters_body}}}}}}}"#);
    let agg = parse_chunks([body.as_bytes()])
        .expect("parses")
        .aggs
        .remove(0);
    assert!(agg.field.is_empty());
    *agg.filters.expect("a filters")
}

fn reason(filters_body: &str) -> String {
    let body = format!(r#"{{"aggs":{{"f":{{"filters":{filters_body}}}}}}}"#);
    parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure()
        .reason
        .clone()
}

fn keys(spec: &FiltersSpec) -> Vec<&str> {
    spec.filters.iter().map(|(k, _)| k.as_str()).collect()
}

fn result(keyed: bool, buckets: &[(&str, u64)]) -> FiltersResult {
    FiltersResult {
        keyed,
        single: false,
        buckets: buckets.iter().map(|(k, n)| ((*k).to_owned(), *n)).collect(),
        ..FiltersResult::default()
    }
}

fn written(result: &FiltersResult) -> String {
    let mut out = Vec::new();
    write_filters(&mut out, result).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

// AC-1
#[test]
fn an_array_of_filters_is_keyed_by_position_in_request_order() {
    let s = spec_of(r#"{"filters":[{"term":{"tag":"b"}},{"term":{"tag":"a"}},{"match_all":{}}]}"#);
    assert!(!s.keyed);
    assert_eq!(s.other_key, None);
    assert_eq!(keys(&s), ["0", "1", "2"]);
    assert!(matches!(s.filters[0].1, Query::Term { .. }));
    assert!(matches!(s.filters[2].1, Query::MatchAll { .. }));
}

#[test]
fn an_object_of_filters_is_keyed_and_sorted_by_key() {
    let s =
        spec_of(r#"{"filters":{"b":{"match_all":{}},"a":{"match_all":{}},"B":{"match_all":{}}}}"#);
    assert!(s.keyed);
    assert_eq!(keys(&s), ["B", "a", "b"]);
}

#[test]
fn the_other_bucket_is_on_by_its_flag_or_its_key() {
    let f = r#""filters":[{"match_all":{}}]"#;
    let other = |extra: &str| spec_of(&format!("{{{f}{extra}}}")).other_key;
    assert_eq!(other(""), None);
    assert_eq!(other(r#","other_bucket":true"#).as_deref(), Some("_other_"));
    assert_eq!(other(r#","other_bucket":false"#), None);
    assert_eq!(
        other(r#","other_bucket_key":"rest""#).as_deref(),
        Some("rest")
    );
    assert_eq!(
        other(r#","other_bucket_key":"rest","other_bucket":true"#).as_deref(),
        Some("rest")
    );
    assert_eq!(
        other(r#","other_bucket_key":"rest","other_bucket":false"#),
        None
    );
}

#[test]
fn bad_bodies_fail_as_opensearch_fails_them() {
    assert_eq!(reason(r#"{"filters":[]}"#), "[filters] cannot be empty.");
    assert_eq!(reason(r#"{"filters":{}}"#), "[filters] cannot be empty.");
    assert_eq!(reason("{}"), "[filters] cannot be empty.");
    assert_eq!(
        reason(r#"{"filters":[{"match_all":{}}],"other_bucket":"yes"}"#),
        "Unknown key for a VALUE_STRING in [f]: [other_bucket]."
    );
    assert_eq!(
        reason(r#"{"filters":[{"match_all":{}}],"size":true}"#),
        "Unknown key for a VALUE_BOOLEAN in [f]: [size]."
    );
    assert_eq!(
        reason(r#"{"filters":[{"match_all":{}}],"other":{}}"#),
        "Unknown key for a START_OBJECT in [f]: [other]."
    );
    assert_eq!(
        reason(r#"{"filters":[{"match_all":{}}],"x":[1]}"#),
        "Unknown key for a START_ARRAY in [f]: [x]."
    );
    assert_eq!(
        reason(r#"{"filters":[{"match_all":{}}],"n":3}"#),
        "Unknown key for a VALUE_NUMBER in [f]: [n]."
    );
    assert!(reason(r#"{"filters":[{"nope":{}}]}"#).contains("unknown query [nope]"));
    assert!(reason(r#"{"filters":{"a":{}}}"#).contains("empty clause"));
}

#[test]
fn a_bucket_sub_aggregation_is_refused_as_not_supported_yet() {
    let body = r#"{"aggs":{"f":{"filters":{"filters":[{"match_all":{}}]},"aggs":{"h":{"percentiles":{"field":"n"}}}}}}"#;
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        failure
            .reason
            .contains("[percentiles] sub-aggregation of a [filters]"),
        "{}",
        failure.reason
    );
}

// AC-5
#[test]
fn counts_add_per_bucket_across_shards() {
    let part = |a: u64, b: u64| Value::Filters(Box::new(result(true, &[("a", a), ("_other_", b)])));
    let Some(Value::Filters(got)) = reduce(&[part(1, 2), part(10, 20), part(100, 200)]) else {
        panic!("no reduce");
    };
    assert_eq!(*got, result(true, &[("a", 111), ("_other_", 222)]));
    let Some(Value::Filters(through)) = crate::aggs::partial::reduce(&[part(2, 3), part(5, 7)])
    else {
        panic!("no dispatch");
    };
    assert_eq!(
        through.buckets,
        [("a".to_owned(), 7), ("_other_".to_owned(), 10)]
    );
}

#[test]
fn a_reduce_needs_parts_of_one_shape() {
    let one = Value::Filters(Box::new(result(false, &[("0", 1)])));
    let other = |keyed: bool, key: &str| Value::Filters(Box::new(result(keyed, &[(key, 1)])));
    assert!(reduce(&[]).is_none());
    assert!(reduce(&[one.clone(), Value::Min(1.0)]).is_none());
    assert!(
        reduce(&[
            one.clone(),
            Value::Filters(Box::new(result(false, &[("0", 1), ("1", 1)])))
        ])
        .is_none()
    );
    assert!(reduce(&[one.clone(), other(false, "1")]).is_none());
    assert!(reduce(&[one.clone(), other(true, "0")]).is_none());
    assert!(reduce(&[one.clone(), other(false, "0")]).is_some());
}

// AC-6
#[test]
fn an_array_result_lists_counts_in_order() {
    assert_eq!(
        written(&result(false, &[("0", 3), ("1", 0), ("_other_", 5)])),
        r#""buckets":[{"doc_count":3},{"doc_count":0},{"doc_count":5}]"#
    );
}

#[test]
fn a_keyed_result_names_each_bucket_by_its_key() {
    assert_eq!(
        written(&result(true, &[("a\"b", 3), ("_other_", 5)])),
        r#""buckets":{"a\"b":{"doc_count":3},"_other_":{"doc_count":5}}"#
    );
}

#[test]
fn a_filters_under_a_terms_is_refused_as_not_supported_yet() {
    let body =
        r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"f":{"percentiles":{"field":"n"}}}}}}"#;
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        failure
            .reason
            .contains("[percentiles] sub-aggregation of a [terms]"),
        "{}",
        failure.reason
    );
}

// AC-1
#[test]
fn a_filter_body_is_one_query_under_the_key_zero() {
    let body = r#"{"aggs":{"f":{"filter":{"term":{"tag":"b"}}}}}"#;
    let agg = parse_chunks([body.as_bytes()])
        .expect("parses")
        .aggs
        .remove(0);
    let s = *agg.filters.expect("a filter");
    assert!(s.single);
    assert!(!s.keyed);
    assert_eq!(s.other_key, None);
    assert_eq!(keys(&s), ["0"]);
    assert!(matches!(s.filters[0].1, Query::Term { .. }));
}

#[test]
fn a_malformed_filter_fails_as_a_malformed_query_does() {
    let body = r#"{"aggs":{"f":{"filter":{"no_such_query":{}}}}}"#;
    assert!(parse_chunks([body.as_bytes()]).is_err());
}

#[test]
fn a_filter_refuses_a_bucket_sub_aggregation_as_not_supported_yet() {
    let body =
        r#"{"aggs":{"f":{"filter":{"match_all":{}},"aggs":{"h":{"percentiles":{"field":"n"}}}}}}"#;
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        failure
            .reason
            .contains("[percentiles] sub-aggregation of a [filter]"),
        "{}",
        failure.reason
    );
}

// AC-5
#[test]
fn a_single_and_a_multiple_result_do_not_reduce_together() {
    let mut single = result(false, &[("0", 1)]);
    single.single = true;
    let multi = result(false, &[("0", 2)]);
    let (a, b) = (
        Value::Filters(Box::new(single)),
        Value::Filters(Box::new(multi)),
    );
    assert!(reduce(&[a.clone(), b]).is_none());
    let Some(Value::Filters(sum)) = reduce(&[a.clone(), a]) else {
        panic!("no reduce");
    };
    assert!(sum.single);
    assert_eq!(sum.buckets, [("0".to_owned(), 2)]);
}

// AC-6
#[test]
fn a_single_result_writes_only_its_count() {
    let mut r = result(false, &[("0", 7)]);
    r.single = true;
    assert_eq!(written(&r), r#""doc_count":7"#);
}

// AC-6
#[test]
fn a_buckets_sub_aggregations_follow_its_count() {
    use crate::aggs::partial::Format;
    let mut r = result(true, &[("a", 3), ("b", 0)]);
    r.sub_meta = vec![("m".to_owned(), Format::Raw)];
    r.subs = vec![vec![Value::Max(9.0)], vec![Value::Max(f64::NEG_INFINITY)]];
    assert_eq!(
        written(&r),
        r#""buckets":{"a":{"doc_count":3,"m":{"value":9.0}},"b":{"doc_count":0,"m":{"value":null}}}"#
    );
    r.keyed = false;
    r.single = true;
    r.buckets.truncate(1);
    assert_eq!(written(&r), r#""doc_count":3,"m":{"value":9.0}"#);
}

// AC-5
#[test]
fn a_reduce_keeps_no_sub_aggregation_when_no_shard_has_one() {
    let Some(Value::Filters(r)) = reduce(&[
        Value::Filters(Box::new(result(false, &[("0", 1)]))),
        Value::Filters(Box::new(result(false, &[("0", 2)]))),
    ]) else {
        panic!("no reduce");
    };
    assert!(r.subs.is_empty());
    assert_eq!(r.buckets, [("0".to_owned(), 3)]);
}

// AC-5
#[test]
fn a_reduce_reduces_each_buckets_sub_aggregations() {
    use crate::aggs::partial::Format;
    let part = |count: u64, min: f64| {
        let mut r = result(false, &[("0", count)]);
        r.sub_meta = vec![("m".to_owned(), Format::Raw)];
        r.subs = vec![vec![Value::Min(min)]];
        Value::Filters(Box::new(r))
    };
    let Some(Value::Filters(r)) = reduce(&[part(1, 5.0), part(2, 3.0)]) else {
        panic!("no reduce");
    };
    assert_eq!(r.buckets, [("0".to_owned(), 3)]);
    assert_eq!(r.subs, [vec![Value::Min(3.0)]]);
}
