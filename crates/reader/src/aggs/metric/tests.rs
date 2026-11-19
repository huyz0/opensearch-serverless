//! Metric aggregations parsed, written out and reduced.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::partial::reduce;
use super::*;
use crate::dsl::{DslError, SearchSource, parse_chunks};

fn source(body: &str) -> Result<SearchSource, DslError> {
    parse_chunks([body.as_bytes()])
}

fn reason(body: &str) -> String {
    let failure = source(body).expect_err("refused").into_failure();
    failure.reason.clone()
}

fn agg(name: &str, metric: Metric, field: &str) -> MetricAgg {
    MetricAgg {
        cardinality: crate::aggs::metric::CardinalityOptions::default(),
        percentiles: crate::aggs::metric::PercentilesOptions::default(),
        top_hits: None,
        terms: None,
        name: name.to_owned(),
        metric,
        field: field.to_owned(),
    }
}

#[test]
fn every_served_metric_parses_in_request_order() {
    let s = source(
        r#"{"size":0,"aggs":{"a":{"min":{"field":"n"}},"b":{"max":{"field":"n"}},
            "c":{"sum":{"field":"n"}},"d":{"avg":{"field":"n"}},
            "e":{"stats":{"field":"n"}},"f":{"value_count":{"field":"n"}}}}"#,
    )
    .expect("parses");
    assert_eq!(
        s.aggs,
        [
            agg("a", Metric::Min, "n"),
            agg("b", Metric::Max, "n"),
            agg("c", Metric::Sum, "n"),
            agg("d", Metric::Avg, "n"),
            agg("e", Metric::Stats, "n"),
            agg("f", Metric::ValueCount, "n"),
        ]
    );
    let aggregations = source(r#"{"aggregations":{"x":{"min":{"field":"y"}}}}"#).expect("parses");
    assert_eq!(aggregations.aggs, [agg("x", Metric::Min, "y")]);
    assert!(source(r#"{"aggs":{}}"#).expect("parses").aggs.is_empty());
    // An empty sub-aggregations object gives no factories, so no refusal.
    let empty_subs = source(r#"{"aggs":{"x":{"aggs":{},"min":{"field":"y"}}}}"#).expect("parses");
    assert_eq!(empty_subs.aggs, [agg("x", Metric::Min, "y")]);
    for m in Metric::ALL {
        assert_eq!(Metric::from_name(m.name()), Some(m));
    }
    assert_eq!(Metric::from_name("histogram"), None);
}

#[test]
fn malformed_aggregations_fail_as_opensearch_fails_them() {
    let cases = [
        (r#"{"aggs":[]}"#, "Unknown key for a START_ARRAY in [aggs]."),
        (
            r#"{"aggs":{"a":1}}"#,
            "Aggregation definition for [a starts with a [VALUE_NUMBER], expected a [START_OBJECT].",
        ),
        (
            r#"{"aggs":{"a[0]":{"min":{"field":"n"}}}}"#,
            "Invalid aggregation name [a[0]]. Aggregation names can contain any character \
             except '[', ']', and '>'",
        ),
        (
            r#"{"aggs":{"a":{}}}"#,
            "Missing definition for aggregation [a]",
        ),
        (
            r#"{"aggs":{"a":{"min":1}}}"#,
            "Expected [START_OBJECT] under [min], but got a [VALUE_NUMBER] in [a]",
        ),
        (
            r#"{"aggs":{"a":{"min":{"field":"n"},"max":{"field":"n"}}}}"#,
            "Found two aggregation type definitions in [a]: [min] and [max]",
        ),
        (
            r#"{"aggs":{"a":{"min":{"field":"n"}},"a":{"max":{"field":"n"}}}}"#,
            "Two sibling aggregations cannot have the same name: [a]",
        ),
        (
            r#"{"aggs":{"a":{"mn":{"field":"n"}}}}"#,
            "Unknown aggregation type [mn] did you mean [min]?",
        ),
        (
            r#"{"aggs":{"a":{"mni":{"field":"n"}}}}"#,
            "Unknown aggregation type [mni]",
        ),
        (
            r#"{"aggs":{"a":{"nope":{}}}}"#,
            "Unknown aggregation type [nope]",
        ),
        (
            r#"{"aggs":{"a":{"histogram":{"field":"tag"}}}}"#,
            "the [histogram] aggregation is not supported by this node yet",
        ),
        (
            r#"{"aggs":{"a":{"min":{"field":"n"},"meta":{}}}}"#,
            "an aggregation's [meta] is not supported by this node yet",
        ),
        (
            r#"{"aggs":{"a":{"min":{"field":"n"},"aggs":{"b":{"max":{"field":"n"}}}}}}"#,
            "Aggregator [a] of type [min] cannot accept sub-aggregations",
        ),
        (
            r#"{"aggs":{"a":{"aggs":{},"aggregations":{},"min":{"field":"n"}}}}"#,
            "Found two sub aggregation definitions under [a]",
        ),
        (
            r#"{"aggs":{"a":{"min":{}}}}"#,
            "Required one of fields [field, script], but none were specified. ",
        ),
        (
            r#"{"aggs":{"a":{"min":{"feild":"n"}}}}"#,
            "[1:22] [min] unknown field [feild] did you mean [field]?",
        ),
        (
            r#"{"aggs":{"a":{"value_count":{"formt":"0","field":"n"}}}}"#,
            "[1:30] [value_count] unknown field [formt] did you mean [format]?",
        ),
        (
            r#"{"aggs":{"a":{"min":{"field":1}}}}"#,
            "[1:30] [min] field doesn't support values of type: VALUE_NUMBER",
        ),
        (
            r#"{"aggs":{"a":{"min":{"field":"n","missing":0}}}}"#,
            "the [min] aggregation's [missing] is not supported by this node yet",
        ),
        (
            r#"{"aggs":{"a":{"sum":{"script":"1"}}}}"#,
            "the [sum] aggregation's [script] is not supported by this node yet",
        ),
        (
            r#"{"aggs":{"a":{"avg":{"format":"0.0","field":"n"}}}}"#,
            "the [avg] aggregation's [format] is not supported by this node yet",
        ),
        (
            r#"{"aggs":{"a":{"min":{"field":"n"},"x":1}}}"#,
            "Expected [START_OBJECT] under [x], but got a [VALUE_NUMBER] in [a]",
        ),
        (
            r#"{"aggs":{"a":{"aggs":{"b":2}}}}"#,
            "Aggregation definition for [b starts with a [VALUE_NUMBER], expected a [START_OBJECT].",
        ),
    ];
    for (body, want) in cases {
        assert_eq!(reason(body), want, "{body}");
    }
}

#[test]
fn a_parse_failure_carries_its_exception_and_location() {
    let failure = source(r#"{"aggs":{"a":{}}}"#)
        .expect_err("refused")
        .into_failure();
    assert_eq!(failure.kind, "parsing_exception");
    let failure = source(r#"{"aggs":{"a":{"mn":{}}}}"#)
        .expect_err("refused")
        .into_failure();
    assert_eq!(
        failure.reason,
        "Unknown aggregation type [mn] did you mean [min]?"
    );
    let cause = failure.cause.as_deref().expect("a cause");
    assert_eq!(cause.kind, "named_object_not_found_exception");
    assert_eq!(cause.reason, "[1:20] unknown field [mn]");
    let failure =
        source(r#"{"aggs":{"a":{"min":{"field":"n"},"aggs":{"b":{"max":{"field":"n"}}}}}}"#)
            .expect_err("refused")
            .into_failure();
    assert_eq!(failure.kind, "aggregation_initialization_exception");
    let e = source(r#"{"aggs":{"a":{"min":{"field":"n"},"aggs":{"b":{"max":{"field":"n"}}}}}}"#)
        .expect_err("refused");
    assert_eq!(e.status(), 500, "OpenSearch keeps the default status");
    assert_eq!(
        source(r#"{"aggs":{"a":{}}}"#)
            .expect_err("refused")
            .status(),
        400
    );
    let failure = source(r#"{"aggs":{"a":{"min":{"feild":"n"}}}}"#)
        .expect_err("refused")
        .into_failure();
    assert_eq!(failure.kind, "x_content_parse_exception");
}

#[test]
fn state_bytes_grow_with_each_aggregation() {
    assert_eq!(state_bytes(&[]), 0);
    let one = state_bytes(&[agg("a", Metric::Min, "n")]);
    assert!(one > std::mem::size_of::<Partial>());
    let two = state_bytes(&[agg("a", Metric::Min, "n"), agg("bb", Metric::Max, "n")]);
    assert_eq!(two, 2 * one + 1);
}

fn written(parts: &[Partial]) -> String {
    let mut out = Vec::new();
    write_aggregations(&mut out, parts).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

fn part(name: &str, format: Format, value: Value) -> Partial {
    Partial {
        name: name.to_owned(),
        format,
        value,
    }
}

#[test]
fn results_are_written_as_opensearch_writes_them() {
    assert_eq!(written(&[]), "");
    let raw = [
        part("a", Format::Raw, Value::Min(1.0)),
        part("b", Format::Raw, Value::Max(f64::NEG_INFINITY)),
        part("c", Format::Raw, Value::Sum(0.0)),
        part("d", Format::Raw, Value::Avg { sum: 3.0, count: 2 }),
        part(
            "e",
            Format::Raw,
            Value::Stats {
                count: 2,
                sum: 3.0,
                min: 1.0,
                max: 2.0,
            },
        ),
        part("f", Format::Raw, Value::ValueCount(7)),
        part("g", Format::Raw, Value::Avg { sum: 0.0, count: 0 }),
        part(
            "h",
            Format::Raw,
            Value::Stats {
                count: 0,
                sum: 0.0,
                min: f64::INFINITY,
                max: f64::NEG_INFINITY,
            },
        ),
    ];
    assert_eq!(
        written(&raw),
        r#","aggregations":{"a":{"value":1.0},"b":{"value":null},"c":{"value":0.0},"d":{"value":1.5},"e":{"count":2,"min":1.0,"max":2.0,"avg":1.5,"sum":3.0},"f":{"value":7},"g":{"value":null},"h":{"count":0,"min":null,"max":null,"avg":null,"sum":0.0}}"#
    );
    let day = 983_664_000_000.0;
    let dated = [
        part("a", Format::Date, Value::Min(day)),
        part("b", Format::Date, Value::Max(f64::NEG_INFINITY)),
        part("c", Format::Date, Value::Sum(day)),
        part("d", Format::Date, Value::Avg { sum: day, count: 1 }),
        part("e", Format::Date, Value::Avg { sum: 0.0, count: 0 }),
        part(
            "f",
            Format::Date,
            Value::Stats {
                count: 1,
                sum: day,
                min: day,
                max: day,
            },
        ),
        part("g", Format::Date, Value::ValueCount(1)),
    ];
    let s = "2001-03-04T00:00:00.000Z";
    assert_eq!(
        written(&dated),
        format!(
            r#","aggregations":{{"a":{{"value":9.83664E11,"value_as_string":"{s}"}},"b":{{"value":null}},"c":{{"value":9.83664E11,"value_as_string":"{s}"}},"d":{{"value":9.83664E11,"value_as_string":"{s}"}},"e":{{"value":null}},"f":{{"count":1,"min":9.83664E11,"max":9.83664E11,"avg":9.83664E11,"sum":9.83664E11,"min_as_string":"{s}","max_as_string":"{s}","avg_as_string":"{s}","sum_as_string":"{s}"}},"g":{{"value":1}}}}"#
        )
    );
}

#[test]
fn results_are_listed_in_hash_map_order() {
    // "b" (98) hashes to bucket 2, "a" (97) to 1: a HashMap lists "a" first.
    let parts = [
        part("b", Format::Raw, Value::ValueCount(2)),
        part("a", Format::Raw, Value::ValueCount(1)),
    ];
    assert_eq!(
        written(&parts),
        r#","aggregations":{"a":{"value":1},"b":{"value":2}}"#
    );
}

#[test]
fn names_are_listed_as_opensearch_listed_them() {
    // Recorded from OpenSearch 3.10.0: put as c, b, a, sum_y, vc, listed
    // a, b, c, vc, sum_y.
    let names = ["c", "b", "a", "sum_y", "vc"];
    let order: Vec<&str> = hash_order::hash_map_order(&names)
        .into_iter()
        .map(|i| names[i])
        .collect();
    assert_eq!(order, ["a", "b", "c", "vc", "sum_y"]);
}

#[test]
fn shard_results_reduce_as_opensearch_reduces_them() {
    assert_eq!(reduce(&[]), None);
    assert_eq!(
        reduce(&[Value::Min(3.0), Value::Min(1.0), Value::Min(f64::INFINITY)]),
        Some(Value::Min(1.0))
    );
    assert_eq!(
        reduce(&[Value::Max(3.0), Value::Max(f64::NEG_INFINITY)]),
        Some(Value::Max(3.0))
    );
    assert_eq!(
        reduce(&[Value::ValueCount(3), Value::ValueCount(4)]),
        Some(Value::ValueCount(7))
    );
    assert_eq!(
        reduce(&[
            Value::Avg { sum: 1.0, count: 1 },
            Value::Avg { sum: 2.0, count: 3 }
        ]),
        Some(Value::Avg { sum: 3.0, count: 4 })
    );
    assert_eq!(
        reduce(&[
            Value::Stats {
                count: 1,
                sum: 5.0,
                min: 5.0,
                max: 5.0
            },
            Value::Stats {
                count: 2,
                sum: 3.0,
                min: 1.0,
                max: 2.0
            },
        ]),
        Some(Value::Stats {
            count: 3,
            sum: 8.0,
            min: 1.0,
            max: 5.0
        })
    );
    // Compensated: a naive sum of 1e16, 1.0 and 1.0 loses both ones.
    assert_eq!(
        reduce(&[Value::Sum(1e16), Value::Sum(1.0), Value::Sum(1.0)]),
        Some(Value::Sum(1e16 + 2.0))
    );
    // A non-finite shard sum is added to the tally, Java's NaN for inf - inf.
    let Some(Value::Sum(nan)) = reduce(&[Value::Sum(f64::INFINITY), Value::Sum(f64::NEG_INFINITY)])
    else {
        panic!("a sum")
    };
    assert_eq!(nan.to_bits(), 0x7ff8_0000_0000_0000);
    assert_eq!(
        reduce(&[Value::Sum(f64::INFINITY), Value::Sum(1.0)]),
        Some(Value::Sum(f64::INFINITY))
    );
    assert_eq!(reduce(&[Value::Min(1.0), Value::Max(2.0)]), None);
}
