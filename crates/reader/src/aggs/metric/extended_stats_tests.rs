//! `extended_stats` parsed, reduced and written.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use crate::aggs::metric::partial::reduce_scalar;
use crate::aggs::metric::render::write_body;
use crate::aggs::partial::{Format, Value};
use crate::dsl::parse_chunks;

fn reason(agg: &str) -> String {
    let body = format!(r#"{{"aggs":{{"e":{{"extended_stats":{agg}}}}}}}"#);
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    failure.reason.clone()
}

fn stats(count: u64, sum: f64, min: f64, max: f64, squares: f64) -> Value {
    Value::ExtendedStats {
        count,
        sum,
        min,
        max,
        sum_of_squares: squares,
        sigma: 2.0,
    }
}

fn written(format: Format, value: &Value) -> String {
    let mut out = Vec::new();
    write_body(&mut out, format, value).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

// AC-4
#[test]
fn options_this_node_does_not_serve_are_refused_by_name() {
    for key in ["missing", "format", "value_type"] {
        let got = reason(&format!(r#"{{"field":"n","{key}":"x"}}"#));
        assert!(
            got.contains(&format!("the [extended_stats] aggregation's [{key}]")),
            "{key}: {got}"
        );
    }
}

// AC-3
#[test]
fn sigma_is_a_number_of_at_least_zero() {
    let body = |sigma: &str| format!(r#"{{"field":"n","sigma":{sigma}}}"#);
    let got = reason(&body("-1"));
    assert!(
        got.contains("[extended_stats] failed to parse field [sigma]"),
        "{got}"
    );
    assert!(reason(&body("\"x\"")).contains("failed to parse field [sigma]"));
    assert!(reason(r#"{"field":"n","nope":1}"#).contains("[extended_stats] unknown field [nope]"));
    let parsed = |sigma: &str| {
        let text = format!(r#"{{"aggs":{{"e":{{"extended_stats":{}}}}}}}"#, body(sigma));
        parse_chunks([text.as_bytes()]).expect("parses").aggs[0].sigma
    };
    assert_eq!(parsed("0"), 0.0);
    assert_eq!(parsed("3"), 3.0);
    assert_eq!(parsed("\"2.5\""), 2.5);
    let default = parse_chunks([br#"{"aggs":{"e":{"extended_stats":{"field":"n"}}}}"#.as_slice()])
        .expect("parses");
    assert_eq!(default.aggs[0].sigma, 2.0);
}

// AC-5
#[test]
fn shards_reduce_to_the_state_of_one_shard_holding_every_value() {
    // 1, 2, 3 on one shard and 4, 5 on the other.
    let a = stats(3, 6.0, 1.0, 3.0, 14.0);
    let b = stats(2, 9.0, 4.0, 5.0, 41.0);
    let reduced = reduce_scalar(&[a, b]).expect("reduces");
    assert_eq!(reduced, stats(5, 15.0, 1.0, 5.0, 55.0));
    assert!(reduce_scalar(&[stats(1, 1.0, 1.0, 1.0, 1.0), Value::ValueCount(1)]).is_none());
}

// AC-1
#[test]
fn no_value_writes_nulls_and_one_value_a_sampling_nan() {
    let none = written(
        Format::Raw,
        &stats(0, 0.0, f64::INFINITY, f64::NEG_INFINITY, 0.0),
    );
    assert!(
        none.starts_with(
            r#""count":0,"min":null,"max":null,"avg":null,"sum":0.0,"sum_of_squares":null"#
        ),
        "{none}"
    );
    let one = written(Format::Raw, &stats(1, 10.5, 10.5, 10.5, 110.25));
    assert!(
        one.contains(r#""variance":0.0,"variance_population":0.0,"variance_sampling":"NaN""#),
        "{one}"
    );
    assert!(
        one.contains(r#""upper_sampling":"NaN","lower_sampling":"NaN""#),
        "{one}"
    );
}

// AC-2
#[test]
fn a_date_field_also_writes_every_value_as_text() {
    let dated = written(Format::Date, &stats(2, 0.0, 0.0, 0.0, 0.0));
    assert!(
        dated.contains(r#""min_as_string":"1970-01-01T00:00:00.000Z""#),
        "{dated}"
    );
    assert!(
        dated.contains(r#""std_deviation_bounds_as_string":{"upper":"1970-01-01T00:00:00.000Z""#),
        "{dated}"
    );
}
