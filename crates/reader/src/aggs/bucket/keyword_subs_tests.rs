//! Sub-aggregations of a keyword `terms`, run on the index of four segments.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::keyword_collect_tests::{keys, shard, terms};
use crate::aggs::bucket::keyword_terms::KeywordTermsResult;
use crate::aggs::partial::Value;

const DOCS: usize = 200;

fn only(r: &KeywordTermsResult, term: &str) -> Vec<Value> {
    let at = r.buckets.iter().position(|b| b.0 == term).expect("term");
    r.subs[at].clone()
}

// AC-10
#[test]
fn metric_sub_aggregations_are_those_of_each_terms_documents() {
    let shard = shard("kw-subs", DOCS);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":["t3","t4"]},"aggs":{
            "lo":{"min":{"field":"n"}},"hi":{"max":{"field":"n"}},"s":{"sum":{"field":"n"}},
            "a":{"avg":{"field":"n"}},"c":{"value_count":{"field":"n"}},
            "st":{"stats":{"field":"n"}}}}}}"#,
    );
    assert_eq!(keys(&got), [("t3", 13), ("t4", 13)]);
    assert_eq!(got.sub_meta.len(), 6);
    let names: Vec<&str> = got.sub_meta.iter().map(|m| m.0.as_str()).collect();
    assert_eq!(names, ["lo", "hi", "s", "a", "c", "st"]);
    // t3: n = 3, 19, ..., 195 (13 values).
    let t3 = only(&got, "t3");
    assert_eq!(t3[0], Value::Min(3.0));
    assert_eq!(t3[1], Value::Max(195.0));
    assert_eq!(t3[2], Value::Sum(1287.0));
    assert_eq!(
        t3[3],
        Value::Avg {
            sum: 1287.0,
            count: 13
        }
    );
    assert_eq!(t3[4], Value::ValueCount(13));
    assert_eq!(
        t3[5],
        Value::Stats {
            count: 13,
            sum: 1287.0,
            min: 3.0,
            max: 195.0
        }
    );
    assert_eq!(only(&got, "t4")[0], Value::Min(4.0));
}

#[test]
fn sub_aggregations_follow_the_querys_matches_and_empty_buckets_are_empty_metrics() {
    let shard = shard("kw-subs-q", DOCS);
    let got = terms(
        &shard,
        r#"{"query":{"range":{"n":{"gte":10,"lt":13}}},"aggs":{"t":{"terms":{
            "field":"tag","min_doc_count":0,"include":["t10","t3"]},
            "aggs":{"s":{"sum":{"field":"n"}},"lo":{"min":{"field":"n"}}}}}}"#,
    );
    assert_eq!(keys(&got), [("t10", 1), ("t3", 0)]);
    assert_eq!(only(&got, "t10"), [Value::Sum(10.0), Value::Min(10.0)]);
    assert_eq!(only(&got, "t3")[0], Value::Sum(0.0));
    assert!(matches!(only(&got, "t3")[1], Value::Min(v) if v == f64::INFINITY));
}

#[test]
fn a_sub_aggregation_of_an_unmapped_field_is_empty_in_every_bucket() {
    let shard = shard("kw-subs-u", DOCS);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","size":2},"aggs":{"x":{"sum":{"field":"nope"}}}}}}"#,
    );
    assert_eq!(got.subs.len(), got.buckets.len());
    assert!(got.subs.iter().all(|s| s == &[Value::Sum(0.0)]));
}

#[test]
fn sub_aggregations_that_are_not_served_are_refused() {
    let shard = shard("kw-subs-r", DOCS);
    for (body, what) in [
        (
            r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"x":{"terms":{"field":"k0"}}}}}}"#,
            "a [terms] sub-aggregation",
        ),
        (
            r#"{"aggs":{"t":{"terms":{"field":"n"},"aggs":{"x":{"sum":{"field":"n"}}}}}}"#,
            "sub-aggregations of a numeric",
        ),
        (
            r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"x":{"min":{"field":"k0"}}}}}}"#,
            "not supported for aggregation [min]",
        ),
    ] {
        let source = crate::dsl::parse_chunks([body.as_bytes()]);
        let message = match source {
            Err(e) => format!("{e:?}"),
            Ok(s) => format!(
                "{:?}",
                crate::search::search(
                    &shard,
                    "idx",
                    &s,
                    &lucene_analysis::Analyzer::standard(None)
                )
                .err()
            ),
        };
        assert!(message.contains(what), "{what}: {message}");
    }
}

// AC-13
#[test]
fn sub_aggregations_reserve_their_states_with_the_terms() {
    use crate::aggs::bucket::keyword_collect::{SUB_COPIES, reserved_bytes};
    let shard = shard("kw-subs-b", DOCS);
    let spec = crate::aggs::bucket::terms::TermsSpec::default();
    let plain = reserved_bytes(&shard, "tag", &spec, 0);
    // 64 dictionary terms in four segments, and `shard_size` 25 of them kept.
    let (terms, kept, subs) = (64, 25, 3);
    assert_eq!(
        reserved_bytes(&shard, "tag", &spec, subs) - plain,
        terms * subs * std::mem::size_of::<lucene_aggs::metric_aggs::MetricState>()
            + kept * subs * SUB_COPIES * std::mem::size_of::<Value>()
    );
}
