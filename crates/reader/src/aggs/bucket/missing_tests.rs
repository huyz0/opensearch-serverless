//! `missing` parsed.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use crate::aggs::metric::Metric;
use crate::dsl::{Query, parse_chunks};

fn parsed(body: &str) -> Result<crate::aggs::metric::MetricAgg, String> {
    let body = format!(r#"{{"aggs":{{"m":{{"missing":{body}}}}}}}"#);
    parse_chunks([body.as_bytes()])
        .map(|mut s| s.aggs.remove(0))
        .map_err(|e| e.into_failure().reason.clone())
}

// AC-1
#[test]
fn a_missing_body_is_the_filter_of_documents_without_the_field() {
    let agg = parsed(r#"{"field":"tag"}"#).expect("parses");
    assert_eq!(agg.metric, Metric::Missing);
    assert_eq!(Metric::from_name("missing"), Some(Metric::Missing));
    assert_eq!(Metric::Missing.name(), "missing");
    let spec = agg.filters.expect("a filter");
    assert!(spec.single);
    assert_eq!(spec.filters.len(), 1);
    assert_eq!(spec.filters[0].0, "0");
    let Query::Bool(b) = &spec.filters[0].1 else {
        panic!("not a bool");
    };
    assert!(b.must.is_empty() && b.should.is_empty() && b.filter.is_empty());
    assert_eq!(b.must_not.len(), 1);
    assert!(matches!(
        &b.must_not[0],
        Query::More(m) if matches!(&**m, crate::dsl::queries::More::Exists { field, .. } if field == "tag")
    ));
}

// AC-1
#[test]
fn a_missing_body_without_a_field_or_with_a_bad_key_fails_as_range_does() {
    let reason = parsed("{}").expect_err("refused");
    assert!(
        reason.contains("Required one of fields [field, script]"),
        "{reason}"
    );
    let reason = parsed(r#"{"field":1}"#).expect_err("refused");
    assert!(
        reason.contains("[missing] field doesn't support values of type: VALUE_NUMBER"),
        "{reason}"
    );
    let reason = parsed(r#"{"field":"a","nope":1}"#).expect_err("refused");
    assert!(
        reason.contains("[missing] unknown field [nope]"),
        "{reason}"
    );
    for key in ["missing", "value_type", "format", "script"] {
        let reason = parsed(&format!(r#"{{"field":"a","{key}":"x"}}"#)).expect_err("refused");
        assert!(
            reason.contains(&format!("the [missing] aggregation's [{key}]"))
                && reason.contains("not supported by this node yet"),
            "{reason}"
        );
    }
}
