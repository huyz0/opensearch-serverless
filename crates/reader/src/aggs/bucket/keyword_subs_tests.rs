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
            r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"x":{"histogram":{"field":"n","interval":5}}}}}}"#,
            "a [histogram] sub-aggregation",
        ),
        (
            r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"x":{"terms":{"field":"n"}}}}}}"#,
            "a [terms] sub-aggregation of a keyword",
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
    use crate::aggs::bucket::keyword_collect::{Parent, SUB_COPIES, reserved_bytes};
    let shard = shard("kw-subs-b", DOCS);
    let spec = crate::aggs::bucket::terms::TermsSpec::default();
    let plain = reserved_bytes(&shard, "tag", &spec, 0, Parent::TOP);
    // 64 dictionary terms in four segments, and `shard_size` 25 of them kept.
    let (terms, kept, subs) = (64, 25, 3);
    assert_eq!(
        reserved_bytes(&shard, "tag", &spec, subs, Parent::TOP) - plain,
        terms * subs * std::mem::size_of::<lucene_aggs::metric_aggs::MetricState>()
            + kept * subs * SUB_COPIES * std::mem::size_of::<Value>()
    );
}

fn inner(r: &KeywordTermsResult, term: &str, at: usize) -> KeywordTermsResult {
    match &only(r, term)[at] {
        Value::KeywordTerms(t) => (**t).clone(),
        other => panic!("not a keyword terms: {other:?}"),
    }
}

// AC-14
#[test]
fn an_inner_terms_counts_within_each_outer_bucket() {
    let shard = shard("kw-nest", DOCS);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":["t3","t4"]},"aggs":{
            "k":{"terms":{"field":"k0","size":2},"aggs":{"hi":{"max":{"field":"n"}}}},
            "m":{"max":{"field":"n"}}}}}}"#,
    );
    assert_eq!(keys(&got), [("t3", 13), ("t4", 13)]);
    // The inner terms is the first sub-aggregation, the metric the second.
    let k = inner(&got, "t3", 0);
    // t3's documents are i = 3, 19, ..., 195; k0 is `v<i % 7>`, so each of
    // the seven values has one or two of them (13 = 6 * 2 + 1).
    let mut counts = [0u64; 7];
    for i in (3..200).step_by(16) {
        counts[i % 7] += 1;
    }
    let mut expected: Vec<(String, u64)> = (0..7).map(|v| (format!("v{v}"), counts[v])).collect();
    expected.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    // The shard keeps `shard_size` (default 14) of 7: all of them.
    assert_eq!(k.buckets.len(), 7);
    assert_eq!(
        k.buckets.iter().map(|b| b.1).sum::<u64>(),
        13,
        "{:?}",
        k.buckets
    );
    let kept = crate::aggs::bucket::keyword_terms::reduce_results(&[&k]);
    assert_eq!(
        kept.buckets,
        expected.iter().take(2).cloned().collect::<Vec<_>>()
    );
    // The inner `max` is of the inner bucket's documents.
    let top = &kept.buckets[0].0;
    let v: usize = top[1..].parse().unwrap();
    let largest = (3..200).step_by(16).filter(|i| i % 7 == v).max().unwrap();
    let at = k.buckets.iter().position(|b| &b.0 == top).unwrap();
    assert_eq!(k.subs[at], [Value::Max(largest as f64)]);
    assert_eq!(only(&got, "t3")[1], Value::Max(195.0));
    // An inner filter and a zero `min_doc_count` apply inside each bucket.
    let got = terms(
        &shard,
        r#"{"query":{"range":{"n":{"lt":20}}},"aggs":{"t":{"terms":{"field":"tag",
            "include":["t3"]},"aggs":{"k":{"terms":{"field":"k0","min_doc_count":0,
            "include":["v1","v3","v6"]}}}}}}"#,
    );
    // t3's documents under 20 are 3 and 19: v3 and v5.
    let k = inner(&got, "t3", 0);
    let listed: Vec<(&str, u64)> = k.buckets.iter().map(|b| (b.0.as_str(), b.1)).collect();
    assert_eq!(listed, [("v1", 0), ("v3", 1), ("v6", 0)]);
}

#[test]
fn an_inner_terms_of_an_unmapped_field_is_empty_and_a_numeric_one_is_refused() {
    let shard = shard("kw-nest-u", DOCS);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","size":2},"aggs":{"k":{"terms":{"field":"nope"}}}}}}"#,
    );
    assert_eq!(got.subs.len(), got.buckets.len());
    for (at, _) in got.buckets.iter().enumerate() {
        let Value::KeywordTerms(k) = &got.subs[at][0] else {
            panic!("{:?}", got.subs[at]);
        };
        assert!(k.buckets.is_empty());
    }
    let body = r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"k":{"terms":{"field":"n"}}}}}}"#;
    let source = crate::dsl::parse_chunks([body.as_bytes()]).expect("parses");
    let e = crate::search::search(
        &shard,
        "idx",
        &source,
        &lucene_analysis::Analyzer::standard(None),
    )
    .expect_err("refused");
    assert!(
        format!("{e:?}").contains("not supported by this node yet"),
        "{e:?}"
    );
}

// AC-14
#[test]
fn terms_nest_three_deep_and_deeper_is_refused() {
    let shard = shard("kw-nest-3", DOCS);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":["t3"]},"aggs":{
            "k":{"terms":{"field":"k0"},"aggs":{"u":{"terms":{"field":"tag"},
            "aggs":{"hi":{"max":{"field":"n"}}}}}}}}}}"#,
    );
    let k = inner(&got, "t3", 0);
    // t3's documents all have tag t3: under each k0 value the one bucket.
    for (at, (_, count)) in k.buckets.iter().enumerate() {
        let Value::KeywordTerms(u) = &k.subs[at][0] else {
            panic!("{:?}", k.subs[at]);
        };
        assert_eq!(u.buckets, [("t3".to_owned(), *count)]);
        assert_eq!(u.subs.len(), 1);
    }
    // Five levels of `terms`, each inside the one before.
    let mut deep = String::from(r#"{"terms":{"field":"tag"}}"#);
    for name in ["d", "c", "b", "a"] {
        deep = format!(r#"{{"terms":{{"field":"tag"}},"aggs":{{"{name}":{deep}}}}}"#);
    }
    let deep = format!(r#"{{"aggs":{{"top":{deep}}}}}"#);
    let e = crate::dsl::parse_chunks([deep.as_bytes()])
        .expect_err("refused")
        .into_failure()
        .reason;
    assert!(e.contains("nested more than 3 levels"), "{e}");
}

// AC-16
#[test]
fn an_inner_terms_reserves_its_pairs_and_buckets() {
    use crate::aggs::bucket::keyword_collect::{BUCKET_BYTES, KEY_COPIES, ORD_BYTES, Parent};
    use crate::aggs::bucket::keyword_subs::reserved;
    use crate::aggs::bucket::{BUCKET_LIMIT, keyword_collect::reserved_bytes};
    let shard = shard("kw-nest-b", DOCS);
    let parse = |body: &str| {
        crate::dsl::parse_chunks([body.as_bytes()])
            .expect("parses")
            .aggs
            .remove(0)
    };
    let flat = parse(r#"{"aggs":{"t":{"terms":{"field":"tag","size":2}}}}"#);
    let nested = parse(
        r#"{"aggs":{"t":{"terms":{"field":"tag","size":2},"aggs":{"k":{"terms":{"field":"k0"}}}}}}"#,
    );
    let top = Parent::TOP;
    let extra = reserved(&shard, &nested, top) - reserved(&shard, &flat, top);
    // The outer field has 64 dictionary terms of four segments and keeps
    // 13 buckets; the inner has 28 terms of two bytes, `shard_size` 25 and
    // a cell for each of the 64 outer terms: 28 * 64 pairs.
    let (terms, segments, longest, cells, kept) = (28, 4, 2, 28 * 64, 13 * 25);
    assert_eq!(
        extra,
        terms * ORD_BYTES
            + cells * BUCKET_BYTES
            + segments * longest * 2
            + kept * (longest * KEY_COPIES + BUCKET_BYTES)
    );
    // Over the bucket limit the cells and the kept buckets stop there.
    let spec = crate::aggs::bucket::terms::TermsSpec::default();
    let wide = Parent {
        cells: 10_000,
        kept: 5_000,
    };
    assert_eq!(
        reserved_bytes(&shard, "k0", &spec, 0, wide),
        terms * ORD_BYTES
            + BUCKET_LIMIT * BUCKET_BYTES
            + segments * longest * 2
            + BUCKET_LIMIT * (longest * KEY_COPIES + BUCKET_BYTES)
    );
    // A zero `min_doc_count` makes every cell a candidate bucket.
    let zero = crate::aggs::bucket::terms::TermsSpec {
        min_doc_count: 0,
        ..spec
    };
    assert_eq!(
        reserved_bytes(&shard, "k0", &zero, 0, wide)
            - reserved_bytes(&shard, "k0", &Default::default(), 0, wide),
        BUCKET_LIMIT * crate::aggs::bucket::keyword_collect::ZERO_BUCKET_BYTES
    );
}
