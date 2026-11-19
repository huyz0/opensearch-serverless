//! `range` parsed, reduced and written out.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::range::{RangeBucket, RangeResult, RangeSpec, reduce};
use super::range_render::write_range;
use crate::aggs::metric::MetricAgg;
use crate::aggs::partial::{Format, Value};
use crate::dsl::parse_chunks;

fn aggs(range_body: &str) -> Vec<MetricAgg> {
    let body = format!(r#"{{"aggs":{{"r":{{"range":{range_body}}}}}}}"#);
    parse_chunks([body.as_bytes()]).expect("parses").aggs
}

fn spec_of(range_body: &str) -> (String, RangeSpec) {
    let agg = aggs(range_body).remove(0);
    (agg.field, *agg.range.expect("a range"))
}

fn reason(range_body: &str) -> String {
    let body = format!(r#"{{"aggs":{{"r":{{"range":{range_body}}}}}}}"#);
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    let mut text = failure.reason.clone();
    if let Some(cause) = &failure.cause {
        text.push_str(" | ");
        text.push_str(&cause.reason);
    }
    text
}

fn bucket(key: Option<&str>, from: f64, to: f64, doc_count: u64) -> RangeBucket {
    RangeBucket {
        key: key.map(str::to_owned),
        from,
        to,
        doc_count,
    }
}

fn written(format: Format, keyed: bool, buckets: Vec<RangeBucket>) -> String {
    let mut out = Vec::new();
    write_range(&mut out, format, &RangeResult { keyed, buckets }).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

const INF: f64 = f64::INFINITY;

// AC-1
#[test]
fn ranges_parse_with_open_ends_and_sort_by_from_then_to() {
    let (field, s) = spec_of(
        r#"{"field":"n","ranges":[{"from":50},{"to":10,"key":"low"},{"from":10,"to":20},{"from":10,"to":15}]}"#,
    );
    assert_eq!(field, "n");
    assert!(!s.keyed);
    let got: Vec<_> = s
        .ranges
        .iter()
        .map(|r| (r.key.as_deref(), r.from, r.to))
        .collect();
    assert_eq!(
        got,
        [
            (Some("low"), -INF, 10.0),
            (None, 10.0, 15.0),
            (None, 10.0, 20.0),
            (None, 50.0, INF),
        ]
    );
}

#[test]
fn equal_ranges_keep_request_order_and_null_ends_are_open() {
    let (_, s) = spec_of(
        r#"{"field":"n","keyed":true,"ranges":[{"key":"b","from":1,"to":2},{"key":"a","from":1,"to":2},{"from":null,"to":null,"key":7}]}"#,
    );
    assert!(s.keyed);
    let keys: Vec<_> = s.ranges.iter().map(|r| r.key.as_deref()).collect();
    assert_eq!(keys, [Some("7"), Some("b"), Some("a")]);
    // A single range object is a list of one.
    let (_, one) = spec_of(r#"{"field":"n","ranges":{"to":3}}"#);
    assert_eq!(one.ranges.len(), 1);
}

#[test]
fn bad_bodies_fail_as_opensearch_fails_them() {
    assert!(
        reason(r#"{"ranges":[{"to":1}]}"#).starts_with("Required one of fields [field, script]")
    );
    assert_eq!(
        reason(r#"{"field":"n"}"#),
        "No [ranges] specified for the [r] aggregation"
    );
    assert_eq!(
        reason(r#"{"field":"n","ranges":[]}"#),
        "No [ranges] specified for the [r] aggregation"
    );
    assert!(
        reason(r#"{"field":"n","ranges":[{"too":1}]}"#).contains("[range] unknown field [too]")
    );
    assert!(reason(r#"{"field":"n","rangez":[]}"#).contains("[range] unknown field [rangez]"));
    assert!(
        reason(r#"{"field":"n","ranges":[3]}"#)
            .contains("ranges doesn't support values of type: VALUE_NUMBER")
    );
    assert!(
        reason(r#"{"field":"n","ranges":[{"from":true}]}"#)
            .contains("from doesn't support values of type: VALUE_BOOLEAN")
    );
    assert!(
        reason(r#"{"field":3,"ranges":[{"to":1}]}"#)
            .contains("field doesn't support values of type: VALUE_NUMBER")
    );
    assert!(
        reason(r#"{"field":"n","ranges":"x"}"#)
            .contains("ranges doesn't support values of type: VALUE_STRING")
    );
    assert!(reason(r#"{"field":"n","keyed":"maybe","ranges":[{"to":1}]}"#).contains("keyed"));
}

#[test]
fn unserved_options_are_refused_as_not_supported_yet() {
    for key in ["missing", "value_type", "format", "script"] {
        let body = format!(r#"{{"field":"n","{key}":"x","ranges":[{{"to":1}}]}}"#);
        assert!(
            reason(&body).contains(&format!("the [range] aggregation's [{key}]")),
            "{key}"
        );
    }
    assert!(reason(r#"{"field":"n","ranges":[{"from":"2020-01-01"}]}"#).contains("given as text"));
    assert!(reason(r#"{"field":"n","ranges":[{"to":"5"}]}"#).contains("[range]'s [to]"));
    let sub = r#"{"aggs":{"r":{"range":{"field":"n","ranges":[{"to":1}]},"aggs":{"m":{"min":{"field":"n"}}}}}}"#;
    let failure = parse_chunks([sub.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        failure.reason.contains("sub-aggregations of a [range]"),
        "{}",
        failure.reason
    );
}

// AC-5
#[test]
fn counts_add_per_range_across_shards() {
    let part = |counts: [u64; 2]| {
        Value::Range(Box::new(RangeResult {
            keyed: false,
            buckets: vec![
                bucket(None, -INF, 5.0, counts[0]),
                bucket(None, 5.0, INF, counts[1]),
            ],
        }))
    };
    let Some(Value::Range(got)) = reduce(&[part([1, 2]), part([10, 20]), part([100, 200])]) else {
        panic!("no reduce");
    };
    let counts: Vec<_> = got.buckets.iter().map(|b| b.doc_count).collect();
    assert_eq!(counts, [111, 222]);
    assert_eq!((got.buckets[0].to, got.buckets[1].from), (5.0, 5.0));
}

#[test]
fn a_reduce_needs_parts_of_one_shape() {
    let one = Value::Range(Box::new(RangeResult {
        keyed: false,
        buckets: vec![bucket(None, 0.0, 1.0, 1)],
    }));
    let two = Value::Range(Box::new(RangeResult {
        keyed: false,
        buckets: vec![bucket(None, 0.0, 1.0, 1), bucket(None, 1.0, 2.0, 1)],
    }));
    assert!(reduce(&[]).is_none());
    assert!(reduce(&[one.clone(), two]).is_none());
    assert!(reduce(&[one.clone(), Value::Min(1.0)]).is_none());
    assert_eq!(reduce(std::slice::from_ref(&one)), Some(one));
}

#[test]
fn the_shared_reduce_dispatches_a_range_to_its_own() {
    let part = |n: u64| {
        Value::Range(Box::new(RangeResult {
            keyed: false,
            buckets: vec![bucket(None, 0.0, 1.0, n)],
        }))
    };
    let Some(Value::Range(got)) = crate::aggs::partial::reduce(&[part(2), part(3)]) else {
        panic!("no reduce");
    };
    assert_eq!(got.buckets[0].doc_count, 5);
}

// AC-6
#[test]
fn an_array_result_is_written_as_opensearch_writes_it() {
    assert_eq!(
        written(
            Format::Raw,
            false,
            vec![
                bucket(None, -INF, 10.0, 3),
                bucket(Some("mid"), 10.0, 20.5, 4),
                bucket(None, 20.5, INF, 0),
            ]
        ),
        r#""buckets":[{"key":"*-10.0","to":10.0,"doc_count":3},{"key":"mid","from":10.0,"to":20.5,"doc_count":4},{"key":"20.5-*","from":20.5,"doc_count":0}]"#
    );
}

#[test]
fn a_keyed_result_names_each_bucket_by_its_key() {
    assert_eq!(
        written(
            Format::Raw,
            true,
            vec![
                bucket(None, -INF, 10.0, 3),
                bucket(Some("mid"), 10.0, 20.0, 4),
                bucket(None, -INF, INF, 7),
            ]
        ),
        r#""buckets":{"*-10.0":{"to":10.0,"doc_count":3},"mid":{"from":10.0,"to":20.0,"doc_count":4},"*-*":{"doc_count":7}}"#
    );
}

#[test]
fn a_date_range_also_writes_its_bounds_as_text() {
    assert_eq!(
        written(
            Format::Date,
            false,
            vec![bucket(None, 0.0, 86_400_000.0, 2)]
        ),
        r#""buckets":[{"key":"1970-01-01T00:00:00.000Z-1970-01-02T00:00:00.000Z","from":0.0,"from_as_string":"1970-01-01T00:00:00.000Z","to":8.64E7,"to_as_string":"1970-01-02T00:00:00.000Z","doc_count":2}]"#
    );
}
