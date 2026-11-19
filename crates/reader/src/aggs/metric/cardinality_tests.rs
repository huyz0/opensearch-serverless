//! `cardinality` parsed, written out and reduced.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::hll::Hll;
use super::*;
use crate::aggs::partial::reduce;
use crate::dsl::{DslError, SearchSource, parse_chunks};

fn source(body: &str) -> Result<SearchSource, DslError> {
    parse_chunks([body.as_bytes()])
}

fn options(body: &str) -> CardinalityOptions {
    let body = format!(r#"{{"aggs":{{"c":{{"cardinality":{{"field":"n"{body}}}}}}}}}"#);
    let s = source(&body).expect("parses");
    assert_eq!(s.aggs.len(), 1);
    assert_eq!(s.aggs[0].metric, Metric::Cardinality);
    assert_eq!(s.aggs[0].field, "n");
    s.aggs[0].cardinality.clone()
}

/// The failure, its cause's reason if any, for `extra` inside the body.
fn refused(extra: &str) -> (String, Option<String>) {
    let body = format!(r#"{{"aggs":{{"c":{{"cardinality":{{"field":"n"{extra}}}}}}}}}"#);
    let f = source(&body).expect_err("refused").into_failure();
    let cause = f.cause.as_deref().map(|c| c.reason.clone());
    (f.reason.clone(), cause)
}

fn threshold(n: Option<i64>) -> CardinalityOptions {
    CardinalityOptions {
        precision_threshold: n,
        execution_hint: None,
    }
}

#[test]
fn options_parse_as_opensearch_coerces_them() {
    assert_eq!(options(""), CardinalityOptions::default());
    assert_eq!(options(r#","precision_threshold":12"#), threshold(Some(12)));
    // A fraction is truncated, as Jackson's and `Numbers.toLong`'s
    // coercion truncates it; a string holds any decimal.
    assert_eq!(options(r#","precision_threshold":1.7"#), threshold(Some(1)));
    assert_eq!(
        options(r#","precision_threshold":"12""#),
        threshold(Some(12))
    );
    assert_eq!(
        options(r#","precision_threshold":"1.5""#),
        threshold(Some(1))
    );
    assert_eq!(
        options(r#","precision_threshold":"1e3""#),
        threshold(Some(1000))
    );
    assert_eq!(
        options(r#","precision_threshold":"+7""#),
        threshold(Some(7))
    );
    assert_eq!(
        options(r#","precision_threshold":".5""#),
        threshold(Some(0))
    );
    assert_eq!(options(r#","precision_threshold":0"#), threshold(Some(0)));
    // The last one wins; `rehash` is read and ignored.
    assert_eq!(
        options(r#","precision_threshold":3,"precision_threshold":4,"rehash":5"#),
        threshold(Some(4))
    );
    assert_eq!(
        options(r#","execution_hint":"DIRECT""#)
            .execution_hint
            .as_deref(),
        Some("DIRECT")
    );
}

#[test]
fn malformed_options_fail_as_opensearch_fails_them() {
    let failed = "[1:64] [cardinality] failed to parse field [precision_threshold]";
    for (extra, reason, cause) in [
        (
            r#","precision_threshold":-1"#,
            failed,
            Some("[precisionThreshold] must be greater than or equal to 0. Found [-1] in [c]"),
        ),
        (
            r#","precision_threshold":"x""#,
            failed,
            Some("For input string: \"x\""),
        ),
        (
            r#","precision_threshold":"1.2.3""#,
            failed,
            Some("For input string: \"1.2.3\""),
        ),
        (
            r#","precision_threshold":"e5""#,
            failed,
            Some("For input string: \"e5\""),
        ),
        (
            r#","precision_threshold":"1e""#,
            failed,
            Some("For input string: \"1e\""),
        ),
        (
            r#","precision_threshold":"1e30""#,
            failed,
            Some("Value [1e30] is out of range for a long"),
        ),
        (
            r#","precision_threshold":92233720368547758070"#,
            failed,
            Some(
                "Numeric value (92233720368547758070) out of range of `long` \
                 (-9223372036854775808 - 9223372036854775807)",
            ),
        ),
        (
            r#","precision_threshold":1e30"#,
            failed,
            Some(
                "Numeric value (1e30) out of range of `long` \
                 (-9223372036854775808 - 9223372036854775807)",
            ),
        ),
        (
            r#","precision_threshold":true"#,
            "[1:64] [cardinality] precision_threshold doesn't support values of type: VALUE_BOOLEAN",
            None,
        ),
        (
            r#","precision_threshold":null"#,
            "[1:64] [cardinality] precision_threshold doesn't support values of type: VALUE_NULL",
            None,
        ),
        (
            r#","execution_hint":5"#,
            "[1:59] [cardinality] execution_hint doesn't support values of type: VALUE_NUMBER",
            None,
        ),
        (
            r#","rehash":true"#,
            "[1:51] [cardinality] rehash doesn't support values of type: VALUE_BOOLEAN",
            None,
        ),
        (
            r#","format":"0.0""#,
            "[1:42] [cardinality] unknown field [format]",
            None,
        ),
        (
            r#","precison_threshold":3"#,
            "[1:42] [cardinality] unknown field [precison_threshold] did you mean \
             [precision_threshold]?",
            None,
        ),
        (
            r#","missing":3"#,
            "the [cardinality] aggregation's [missing] is not supported by this node yet",
            None,
        ),
    ] {
        let (r, c) = refused(extra);
        assert_eq!(r, reason, "{extra}");
        assert_eq!(c.as_deref(), cause, "{extra}");
    }
}

#[test]
fn the_precision_follows_the_threshold() {
    assert_eq!(threshold(None).precision(), Ok(DEFAULT_PRECISION));
    assert_eq!(threshold(Some(100)).precision(), Ok(10));
    assert!(threshold(Some(i64::MAX)).precision().is_err());
}

#[test]
fn a_cardinality_reserves_its_sketch_only_once_it_can_have_one() {
    let agg = |options: CardinalityOptions| MetricAgg {
        name: "c".to_owned(),
        metric: Metric::Cardinality,
        field: "n".to_owned(),
        cardinality: options,
        percentiles: crate::aggs::metric::PercentilesOptions::default(),
        top_hits: None,
        terms: None,
        histogram: None,
        date_histogram: None,
    };
    let plain = state_bytes(&[MetricAgg {
        metric: Metric::Min,
        ..agg(threshold(None))
    }]);
    assert_eq!(
        state_bytes(&[agg(threshold(None))]),
        plain + hll::sketch_bytes(14)
    );
    assert_eq!(state_bytes(&[agg(threshold(Some(i64::MAX)))]), plain);
}

fn sketch(values: std::ops::Range<i64>, p: u32) -> Hll {
    let mut s = lucene_aggs::cardinality_sketch::Sketch::new(p).expect("precision");
    for v in values {
        s.collect(lucene_aggs::cardinality_sketch::mix64(v));
    }
    let mut bytes = Vec::new();
    s.write_to(&mut bytes);
    Hll::read(&bytes).expect("reads")
}

fn written(value: Value) -> String {
    let mut out = Vec::new();
    write_aggregations(
        &mut out,
        &[Partial {
            name: "c".to_owned(),
            format: Format::Raw,
            value,
        }],
    )
    .expect("writes");
    String::from_utf8(out).expect("utf-8")
}

#[test]
fn a_cardinality_is_written_as_its_estimate() {
    assert_eq!(
        written(Value::Cardinality(Some(sketch(0..200, 14)))),
        r#","aggregations":{"c":{"value":200}}"#
    );
    assert_eq!(
        written(Value::Cardinality(None)),
        r#","aggregations":{"c":{"value":0}}"#
    );
}

#[test]
fn cardinalities_reduce_as_opensearch_reduces_them() {
    let merged = reduce(&[
        Value::Cardinality(None),
        Value::Cardinality(Some(sketch(0..100, 14))),
        Value::Cardinality(Some(sketch(50..150, 14))),
    ]);
    match merged {
        Some(Value::Cardinality(Some(s))) => assert_eq!(s.cardinality(), 150),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        reduce(&[Value::Cardinality(None), Value::Cardinality(None)]),
        Some(Value::Cardinality(None))
    );
    // Precisions that differ, or another kind, do not reduce.
    assert_eq!(
        reduce(&[
            Value::Cardinality(Some(sketch(0..10, 14))),
            Value::Cardinality(Some(sketch(0..10, 13))),
        ]),
        None
    );
    assert_eq!(
        reduce(&[Value::Cardinality(None), Value::ValueCount(1)]),
        None
    );
    assert_eq!(
        reduce(&[Value::ValueCount(1), Value::Cardinality(None)]),
        None
    );
}
