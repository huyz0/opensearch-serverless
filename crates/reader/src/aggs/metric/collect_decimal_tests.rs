//! Metric aggregations over `float` and `double` fields, on a real index of four segments.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use oss_blob::Emulator;
use oss_testkit::index::Spec;

use super::tests::{DOCS, run, shard_of, values};
use crate::OpenShard;
use crate::aggs::metric::Value;

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
fn a_bucket_aggregation_of_a_decimal_field_is_refused_for_now() {
    let shard = decimals_shard("aggs-double-terms");
    let err = run(&shard, r#"{"aggs":{"t":{"terms":{"field":"d"}}}}"#).expect_err("refused");
    assert_eq!(err.status, 400);
    assert!(
        format!("{:?}", err.failure)
            .contains("[terms] of the double field [d] is not supported by this node yet"),
        "{:?}",
        err.failure
    );
}
