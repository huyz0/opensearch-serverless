//! `nested` and `reverse_nested` parsed (`NestedAggregationBuilder.parse`,
//! `ReverseNestedAggregationBuilder.parse`).
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use crate::aggs::metric::{Metric, MetricAgg};
use crate::dsl::parse_chunks;

fn parsed(body: &str) -> MetricAgg {
    parse_chunks([body.as_bytes()])
        .expect("parses")
        .aggs
        .remove(0)
}

fn reason(body: &str) -> String {
    parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure()
        .reason
        .clone()
}

// AC-1
#[test]
fn a_nested_reads_its_path_and_its_sub_aggregations() {
    let agg = parsed(
        r#"{"aggs":{"n":{"nested":{"path":"comments"},"aggs":{"s":{"sum":{"field":"comments.stars"}}}}}}"#,
    );
    assert_eq!(agg.metric, Metric::Nested);
    assert_eq!(agg.name, "n");
    let spec = agg.nested.as_deref().expect("a nested");
    assert_eq!(spec.path.as_deref(), Some("comments"));
    assert_eq!(agg.subs.len(), 1);
    assert_eq!(agg.subs[0].name, "s");
}

// AC-1
#[test]
fn a_nested_without_a_path_is_refused_by_name() {
    assert_eq!(
        reason(r#"{"aggs":{"n":{"nested":{}}}}"#),
        "Missing [path] field for nested aggregation [n]"
    );
}

// AC-1, AC-2
#[test]
fn an_unknown_key_is_refused_with_its_token_and_name() {
    assert_eq!(
        reason(r#"{"aggs":{"n":{"nested":{"path":"c","size":3}}}}"#),
        "Unknown key for a VALUE_NUMBER in [n]: [size]."
    );
    assert_eq!(
        reason(r#"{"aggs":{"n":{"nested":{"pat":"c"}}}}"#),
        "Unknown key for a VALUE_STRING in [n]: [pat]."
    );
    assert_eq!(
        reason(r#"{"aggs":{"r":{"reverse_nested":{"x":true}}}}"#),
        "Unknown key for a VALUE_BOOLEAN in [r]: [x]."
    );
}

// AC-2
#[test]
fn a_reverse_nested_has_an_optional_path() {
    let with = parsed(r#"{"aggs":{"r":{"reverse_nested":{"path":"a"}}}}"#);
    assert_eq!(with.metric, Metric::ReverseNested);
    assert_eq!(
        with.nested.as_deref().expect("a spec").path.as_deref(),
        Some("a")
    );
    let without = parsed(r#"{"aggs":{"r":{"reverse_nested":{}}}}"#);
    assert_eq!(without.nested.as_deref().expect("a spec").path, None);
}

// AC-2
#[test]
fn a_reverse_nested_holds_sub_aggregations() {
    let agg = parsed(
        r#"{"aggs":{"n":{"nested":{"path":"c"},"aggs":{"r":{"reverse_nested":{},"aggs":{"a":{"avg":{"field":"price"}}}}}}}}"#,
    );
    assert_eq!(agg.subs[0].metric, Metric::ReverseNested);
    assert_eq!(agg.subs[0].subs[0].name, "a");
}
