//! `histogram` parsed, reduced, written out and run on a real index.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::histogram::{HistogramOrder, HistogramResult, HistogramSpec, reduce, reduce_results};
use super::histogram_render::write_histogram;
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::metric::MetricAgg;
use crate::aggs::partial::{Format, Value};
use crate::dsl::parse_chunks;

fn aggs(body: &str) -> Vec<MetricAgg> {
    parse_chunks([body.as_bytes()]).expect("parses").aggs
}

fn spec_of(histogram_body: &str) -> (String, HistogramSpec) {
    let body = format!(r#"{{"aggs":{{"h":{{"histogram":{histogram_body}}}}}}}"#);
    let agg = aggs(&body).remove(0);
    (agg.field, *agg.histogram.expect("a histogram"))
}

fn reason(histogram_body: &str) -> String {
    let body = format!(r#"{{"aggs":{{"h":{{"histogram":{histogram_body}}}}}}}"#);
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

fn spec(interval: f64) -> HistogramSpec {
    HistogramSpec {
        interval,
        ..HistogramSpec::default()
    }
}

fn shard_result(spec: &HistogramSpec, buckets: &[(f64, u64)]) -> HistogramResult {
    HistogramResult {
        spec: spec.clone(),
        buckets: buckets.to_vec(),
    }
}

fn reduced(parts: &[HistogramResult]) -> HistogramResult {
    let refs: Vec<&HistogramResult> = parts.iter().collect();
    reduce_results(&refs, BUCKET_LIMIT).expect("within the limit")
}

fn written(format: Format, result: &HistogramResult) -> String {
    let mut out = Vec::new();
    write_histogram(&mut out, format, result).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

// AC-1
#[test]
fn defaults_are_opensearchs() {
    let (field, s) = spec_of(r#"{"field":"n","interval":10}"#);
    assert_eq!(field, "n");
    assert_eq!(s, spec(10.0));
    assert_eq!(s.min_doc_count, 0);
    assert_eq!(s.order, HistogramOrder::KeyAsc);
}

#[test]
fn every_served_key_parses() {
    let (_, s) = spec_of(
        r#"{"field":"n","interval":"2.5","offset":1,"keyed":true,"min_doc_count":3,
            "extended_bounds":{"min":0,"max":9},"hard_bounds":{"min":-1,"max":null},
            "order":[{"_count":"asc"}]}"#,
    );
    assert_eq!(s.interval, 2.5);
    assert_eq!(s.offset, 1.0);
    assert!(s.keyed);
    assert_eq!(s.min_doc_count, 3);
    assert_eq!(s.extended_bounds, Some((Some(0.0), Some(9.0))));
    assert_eq!(s.hard_bounds, (Some(-1.0), None));
    assert_eq!(s.order, HistogramOrder::CountAsc);
}

#[test]
fn orders_parse_by_key_and_count() {
    for (text, want) in [
        (r#"{"_key":"asc"}"#, HistogramOrder::KeyAsc),
        (r#"{"_key":"desc"}"#, HistogramOrder::KeyDesc),
        (r#"{"_count":"asc"}"#, HistogramOrder::CountAsc),
        (r#"{"_count":"desc"}"#, HistogramOrder::CountDesc),
        (r#"{"_term":"desc"}"#, HistogramOrder::KeyDesc),
    ] {
        let (_, s) = spec_of(&format!(r#"{{"field":"n","interval":1,"order":{text}}}"#));
        assert_eq!(s.order, want, "{text}");
    }
}

#[test]
fn bad_requests_fail_with_opensearchs_messages() {
    assert!(
        reason(r#"{"field":"n","interval":0}"#)
            .contains("[interval] must be >0 for histogram aggregation [h]")
    );
    assert!(reason(r#"{"field":"n","interval":-2}"#).contains("[interval] must be >0"));
    assert!(
        reason(r#"{"field":"n","interval":1,"min_doc_count":-1}"#)
            .contains("[minDocCount] must be greater than or equal to 0. Found [-1] in [h]")
    );
    assert!(
        reason(r#"{"field":"n","interval":1,"extended_bounds":{"min":5,"max":1}}"#)
            .contains("max bound [1.0] must be greater than min bound [5.0]")
    );
    assert!(
        reason(
            r#"{"field":"n","interval":1,"extended_bounds":{"min":0,"max":10},
                "hard_bounds":{"min":0,"max":5}}"#
        )
        .contains("Extended bounds have to be inside hard bounds, hard bounds: [0.0--5.0]")
    );
    assert!(
        reason(
            r#"{"field":"n","interval":1,"extended_bounds":{"min":0,"max":10},
                "hard_bounds":{"min":2,"max":20}}"#
        )
        .contains("Extended bounds have to be inside hard bounds")
    );
    assert!(reason(r#"{"field":"n","interval":1,"bogus":1}"#).contains("unknown field [bogus]"));
    assert!(reason(r#"{"interval":1}"#).contains("Required one of fields [field, script]"));
    assert!(reason(r#"{"field":"n","interval":[1]}"#).contains("doesn't support values of type"));
}

#[test]
fn unserved_keys_and_orders_are_refused_as_not_supported_yet() {
    for body in [
        r#"{"field":"n","interval":1,"missing":0}"#,
        r#"{"field":"n","interval":1,"format":"yyyy"}"#,
        r#"{"field":"n","interval":1,"order":{"sub":"asc"}}"#,
        r#"{"field":"n","interval":1,"order":[{"_key":"asc"},{"_count":"asc"}]}"#,
        r#"{"field":"n","interval":1,"order":{"_key":"asc","_count":"asc"}}"#,
    ] {
        assert!(
            reason(body).contains("not supported by this node yet"),
            "{body}"
        );
    }
    let body = br#"{"aggs":{"h":{"histogram":{"field":"n","interval":1},"aggs":{"m":{"max":{"field":"n"}}}}}}"#;
    let e = parse_chunks([&body[..]])
        .expect_err("refused")
        .into_failure();
    assert!(
        e.reason.contains("sub-aggregations of a [histogram]"),
        "{e:?}"
    );
}

// AC-5
#[test]
fn equal_keys_add_and_gaps_fill_with_empty_buckets() {
    let s = spec(10.0);
    let a = shard_result(&s, &[(0.0, 2), (10.0, 1)]);
    let b = shard_result(&s, &[(10.0, 4), (40.0, 1)]);
    assert_eq!(
        reduced(&[a, b]).buckets,
        [(0.0, 2), (10.0, 5), (20.0, 0), (30.0, 0), (40.0, 1)]
    );
}

#[test]
fn min_doc_count_above_zero_drops_buckets_and_makes_no_gaps() {
    let mut s = spec(10.0);
    s.min_doc_count = 2;
    let a = shard_result(&s, &[(0.0, 2), (10.0, 1), (30.0, 3)]);
    assert_eq!(reduced(&[a]).buckets, [(0.0, 2), (30.0, 3)]);
}

#[test]
fn a_shard_with_no_bucket_adds_none() {
    let s = spec(10.0);
    let a = shard_result(&s, &[]);
    let b = shard_result(&s, &[(5.0, 1)]);
    assert_eq!(reduced(&[a.clone(), b]).buckets, [(5.0, 1)]);
    assert!(reduced(&[a]).buckets.is_empty());
}

#[test]
fn extended_bounds_fill_before_and_after_the_data() {
    let mut s = spec(10.0);
    s.extended_bounds = Some((Some(-15.0), Some(35.0)));
    let a = shard_result(&s, &[(10.0, 1)]);
    assert_eq!(
        reduced(&[a]).buckets,
        [
            (-20.0, 0),
            (-10.0, 0),
            (0.0, 0),
            (10.0, 1),
            (20.0, 0),
            (30.0, 0)
        ]
    );
    // With no data the bounds alone make the buckets.
    let empty = shard_result(&s, &[]);
    assert_eq!(reduced(&[empty]).buckets.len(), 6);
    // A bound given on one end fills that end only.
    s.extended_bounds = Some((None, Some(30.0)));
    let a = shard_result(&s, &[(10.0, 1)]);
    assert_eq!(reduced(&[a]).buckets, [(10.0, 1), (20.0, 0), (30.0, 0)]);
    s.extended_bounds = Some((Some(0.0), None));
    let a = shard_result(&s, &[(10.0, 1)]);
    assert_eq!(reduced(&[a]).buckets, [(0.0, 0), (10.0, 1)]);
}

#[test]
fn an_offset_moves_the_fill() {
    let mut s = spec(10.0);
    s.offset = 3.0;
    let a = shard_result(&s, &[(3.0, 1), (23.0, 1)]);
    assert_eq!(reduced(&[a]).buckets, [(3.0, 1), (13.0, 0), (23.0, 1)]);
}

#[test]
fn orders_list_as_opensearch_lists_them() {
    let mut s = spec(1.0);
    let part = |s: &HistogramSpec| shard_result(s, &[(0.0, 5), (1.0, 2), (2.0, 9), (3.0, 7)]);
    s.order = HistogramOrder::KeyDesc;
    assert_eq!(
        reduced(&[part(&s)]).buckets,
        [(3.0, 7), (2.0, 9), (1.0, 2), (0.0, 5)]
    );
    s.order = HistogramOrder::CountDesc;
    assert_eq!(
        reduced(&[part(&s)]).buckets,
        [(2.0, 9), (3.0, 7), (0.0, 5), (1.0, 2)]
    );
    s.order = HistogramOrder::CountAsc;
    assert_eq!(
        reduced(&[part(&s)]).buckets,
        [(1.0, 2), (0.0, 5), (3.0, 7), (2.0, 9)]
    );
}

#[test]
fn zero_keys_of_either_sign_stay_apart_as_double_compare_has_them() {
    let s = spec(1.0);
    let a = shard_result(&s, &[(-0.0, 1), (0.0, 2)]);
    let b = shard_result(&s, &[(0.0, 3)]);
    let got = reduced(&[a, b]).buckets;
    assert_eq!(got.len(), 2);
    assert!(got[0].0.is_sign_negative() && got[0].1 == 1);
    assert_eq!(got[1], (0.0, 5));
}

#[test]
fn a_fill_past_the_bucket_limit_is_refused_not_made() {
    let s = spec(1.0);
    let a = shard_result(&s, &[(0.0, 1), (1_000_000.0, 1)]);
    let err = reduce_results(&[&a], BUCKET_LIMIT).expect_err("too many");
    assert_eq!(err, BUCKET_LIMIT + 1);
    // Exactly at the limit is allowed.
    let a = shard_result(&s, &[(0.0, 1), (65_534.0, 1)]);
    assert_eq!(
        reduce_results(&[&a], BUCKET_LIMIT)
            .expect("fits")
            .buckets
            .len(),
        BUCKET_LIMIT
    );
    // And `reduce` answers none rather than a partial fill.
    let over = Value::Histogram(Box::new(shard_result(&s, &[(0.0, 1), (1_000_000.0, 1)])));
    assert!(reduce(&[over]).is_none());
}

#[test]
fn a_tiny_interval_at_a_huge_key_cannot_loop() {
    // `key + 1.5 * interval` rounds back to the key: the limit stops it.
    let s = spec(1e-9);
    let a = shard_result(&s, &[(1e18, 1), (2e18, 1)]);
    assert!(reduce_results(&[&a], 10).is_err());
}

#[test]
fn reduce_picks_only_histograms() {
    let s = spec(1.0);
    let h = Value::Histogram(Box::new(shard_result(&s, &[(1.0, 1)])));
    assert!(reduce(&[]).is_none());
    assert!(reduce(&[h.clone(), Value::Min(1.0)]).is_none());
    let Some(Value::Histogram(got)) = crate::aggs::partial::reduce(&[h.clone(), h]) else {
        panic!("a histogram");
    };
    assert_eq!(got.buckets, [(1.0, 2)]);
}

// AC-6
#[test]
fn an_array_of_buckets_is_written_as_opensearch_writes_it() {
    let s = spec(2.5);
    let r = shard_result(&s, &[(0.0, 1), (5.0, 3)]);
    assert_eq!(
        written(Format::Raw, &r),
        r#""buckets":[{"key":0.0,"doc_count":1},{"key":2.5,"doc_count":0},{"key":5.0,"doc_count":3}]"#
    );
    assert_eq!(
        written(Format::Raw, &shard_result(&s, &[])),
        r#""buckets":[]"#
    );
}

#[test]
fn keyed_buckets_are_an_object_by_the_keys_text() {
    let mut s = spec(1.0);
    s.keyed = true;
    let r = shard_result(&s, &[(1.0, 1), (2.0, 2)]);
    assert_eq!(
        written(Format::Raw, &r),
        r#""buckets":{"1.0":{"key":1.0,"doc_count":1},"2.0":{"key":2.0,"doc_count":2}}"#
    );
}

// AC-4
#[test]
fn a_date_fields_keys_carry_text_before_the_key() {
    let mut s = spec(86_400_000.0);
    let r = shard_result(&s, &[(0.0, 1)]);
    assert_eq!(
        written(Format::Date, &r),
        r#""buckets":[{"key_as_string":"1970-01-01T00:00:00.000Z","key":0.0,"doc_count":1}]"#
    );
    s.keyed = true;
    let r = shard_result(&s, &[(86_400_000.0, 1)]);
    assert_eq!(
        written(Format::Date, &r),
        r#""buckets":{"1970-01-02T00:00:00.000Z":{"key_as_string":"1970-01-02T00:00:00.000Z","key":8.64E7,"doc_count":1}}"#
    );
}

#[test]
fn a_result_over_the_bucket_limit_fails_to_write() {
    let s = spec(1.0);
    let r = shard_result(&s, &[(0.0, 1), (1_000_000.0, 1)]);
    let mut out = Vec::new();
    assert!(write_histogram(&mut out, Format::Raw, &r).is_err());
}
