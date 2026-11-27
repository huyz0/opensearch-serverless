//! Metric aggregations over `float` and `double` fields, on a real index of four segments.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use oss_blob::Emulator;
use oss_testkit::index::Spec;

use super::tests::{DOCS, run, shard_of, source, values};
use crate::OpenShard;
use crate::aggs::metric::Value;
use crate::aggs::partial::Format;
use crate::search::search;
use lucene_analysis::Analyzer;

/// Sums as OpenSearch's compensated sum adds them, in document order.
fn compensated(values: impl Iterator<Item = f64>) -> f64 {
    let (mut sum, mut c) = (0.0_f64, 0.0_f64);
    for v in values {
        let y = v - c;
        let t = sum + y;
        c = (t - sum) - y;
        sum = t;
    }
    sum
}

fn decimals_shard(name: &str) -> OpenShard<Emulator> {
    shard_of(
        Spec::new(name, DOCS)
            .segments_of(50)
            .with_texts(1)
            .with_decimals(),
    )
}

#[test]
fn metrics_of_a_double_field_across_segments() {
    let shard = decimals_shard("aggs-double");
    let got = values(
        &shard,
        r#"{"size":0,"aggs":{"a":{"min":{"field":"d"}},"b":{"max":{"field":"d"}},
            "c":{"sum":{"field":"d"}},"e":{"stats":{"field":"d"}},"f":{"value_count":{"field":"d"}}}}"#,
    );
    // Document i holds i / 4 - 5: exact in a double.
    let sum = compensated((0..DOCS).map(oss_testkit::index::doc_double));
    assert_eq!(
        got,
        [
            Value::Min(-5.0),
            Value::Max(44.75),
            Value::Sum(sum),
            Value::Stats {
                count: 200,
                sum,
                min: -5.0,
                max: 44.75
            },
            Value::ValueCount(200),
        ]
    );
}

#[test]
fn metrics_of_a_float_field_widen_each_value_to_a_double() {
    let shard = decimals_shard("aggs-float");
    let got = values(
        &shard,
        r#"{"aggs":{"a":{"min":{"field":"f"}},"b":{"max":{"field":"f"}},
            "c":{"sum":{"field":"f"}},"d":{"avg":{"field":"f"}}}}"#,
    );
    let widened = || (0..DOCS).map(|i| f64::from(oss_testkit::index::doc_float(i)));
    let sum = compensated(widened());
    // 19.9f is not 19.9: the float's own value, as OpenSearch reads it.
    assert_ne!(f64::from(oss_testkit::index::doc_float(199)), 19.9);
    assert_eq!(
        got,
        [
            Value::Min(0.0),
            Value::Max(f64::from(oss_testkit::index::doc_float(199))),
            Value::Sum(sum),
            Value::Avg { sum, count: 200 },
        ]
    );
}

#[test]
fn a_decimal_field_aggregates_the_query_matches_only() {
    let shard = decimals_shard("aggs-double-query");
    let got = values(
        &shard,
        r#"{"query":{"range":{"n":{"gte":4,"lt":8}}},
            "aggs":{"s":{"stats":{"field":"d"}},"p":{"percentiles":{"field":"d","percents":[50]}}}}"#,
    );
    // Documents 4 to 7 hold -4, -3.75, -3.5, -3.25.
    assert_eq!(
        got[0],
        Value::Stats {
            count: 4,
            sum: -14.5,
            min: -4.0,
            max: -3.25
        }
    );
    assert!(matches!(got[1], Value::Percentiles { .. }));
}

#[test]
fn a_composite_over_a_decimal_field_is_refused_for_now() {
    let shard = decimals_shard("aggs-double-composite");
    let err = run(
        &shard,
        r#"{"aggs":{"c":{"composite":{"sources":[{"s":{"terms":{"field":"d"}}}]}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(err.status, 400);
    assert_eq!(
        err.failure.reason,
        "aggregating a [double] field [d] is not supported by this node yet"
    );
}

#[test]
fn terms_of_a_double_key_each_bucket_by_its_value_and_write_the_double() {
    let shard = decimals_shard("aggs-double-terms");
    let source = source(
        r#"{"size":0,"aggs":{"t":{"terms":{"field":"d","size":2,"order":{"_key":"desc"}}}}}"#,
    );
    let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    let part = hits.aggregations.first().expect("an aggregation");
    assert_eq!(part.format, Format::Double);
    let Value::Terms(terms) = &part.value else {
        panic!("not a terms");
    };
    assert_eq!(terms.other_doc_count, 198);
    let mut out = Vec::new();
    crate::aggs::bucket::terms_render::write_terms(&mut out, part.format, terms).expect("writes");
    assert_eq!(
        String::from_utf8(out).expect("utf-8"),
        r#""doc_count_error_upper_bound":0,"sum_other_doc_count":198,"buckets":[{"key":44.75,"doc_count":1},{"key":44.5,"doc_count":1}]"#
    );
}

#[test]
fn terms_of_a_float_key_by_the_widened_double() {
    let shard = decimals_shard("aggs-float-terms");
    let source = source(
        r#"{"size":0,"aggs":{"t":{"terms":{"field":"f","size":1,"order":{"_key":"asc"}}}}}"#,
    );
    let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    let part = hits.aggregations.first().expect("an aggregation");
    let Value::Terms(terms) = &part.value else {
        panic!("not a terms");
    };
    let mut out = Vec::new();
    crate::aggs::bucket::terms_render::write_terms(&mut out, part.format, terms).expect("writes");
    assert_eq!(
        String::from_utf8(out).expect("utf-8"),
        r#""doc_count_error_upper_bound":0,"sum_other_doc_count":199,"buckets":[{"key":0.0,"doc_count":1}]"#
    );
}

fn histogram_of(shard: &OpenShard<Emulator>, body: &str) -> Vec<(f64, u64)> {
    match values(shard, body).into_iter().next() {
        Some(Value::Histogram(h)) => h.buckets,
        other => panic!("not a histogram: {other:?}"),
    }
}

#[test]
fn a_histogram_of_a_double_keys_each_bucket_by_its_floor() {
    let shard = decimals_shard("aggs-double-histogram");
    // Document i holds i / 4 - 5: four documents to each whole number from -5 to 44.
    let got = histogram_of(
        &shard,
        r#"{"size":0,"aggs":{"h":{"histogram":{"field":"d","interval":1}}}}"#,
    );
    let want: Vec<(f64, u64)> = (-5..45).map(|k| (f64::from(k), 4)).collect();
    assert_eq!(got, want);
}

#[test]
fn a_histogram_of_a_float_reads_each_value_as_its_float() {
    let shard = decimals_shard("aggs-float-histogram");
    let got = histogram_of(
        &shard,
        r#"{"aggs":{"h":{"histogram":{"field":"f","interval":5}}}}"#,
    );
    assert_eq!(got, [(0.0, 50), (5.0, 50), (10.0, 50), (15.0, 50)]);
}

#[test]
fn a_range_of_a_double_counts_the_documents_in_each_range() {
    let shard = decimals_shard("aggs-double-range");
    let got = values(
        &shard,
        r#"{"size":0,"aggs":{"r":{"range":{"field":"d","ranges":[{"to":0},{"from":0,"to":10},{"from":10}]}}}}"#,
    );
    let Some(Value::Range(r)) = got.into_iter().next() else {
        panic!("not a range");
    };
    let counts: Vec<u64> = r.buckets.iter().map(|b| b.doc_count).collect();
    assert_eq!(counts, [20, 40, 140]);
}

#[test]
fn a_date_aggregation_of_a_double_fails_as_opensearch_fails_it() {
    let shard = decimals_shard("aggs-double-date");
    let err = run(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"d","calendar_interval":"day"}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(err.status, 400);
    assert_eq!(
        err.failure.reason,
        "Field [d] of type [double] is not supported for aggregation [date_histogram]"
    );
}
