//! A `composite` run on a real index of four segments.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::composite::{CompositeResult, KeyPart, reduce_results};
use super::composite_render::write_composite;
use super::keyword_collect_tests::{run, shard};
use crate::OpenShard;
use crate::aggs::partial::Value;
use oss_blob::Emulator;

/// `tag` is `t<i % 16>` and `n` is `i`, two hundred documents.
const DOCS: usize = 200;

fn composite(shard: &OpenShard<Emulator>, aggs: &str) -> CompositeResult {
    let body = format!(r#"{{"size":0,"aggs":{{"c":{{"composite":{aggs}}}}}}}"#);
    let mut parts = run(shard, &body).expect("searches");
    match parts.remove(0).value {
        Value::Composite(c) => *c,
        other => panic!("not a composite: {other:?}"),
    }
}

fn refused(shard: &OpenShard<Emulator>, aggs: &str) -> (u16, String) {
    let body = format!(r#"{{"size":0,"aggs":{{"c":{{"composite":{aggs}}}}}}}"#);
    let e = run(shard, &body).expect_err("refused");
    (e.status, e.failure.reason.clone())
}

fn terms(r: &CompositeResult) -> Vec<(String, u64)> {
    r.buckets
        .iter()
        .map(|(k, c)| match &k[0] {
            KeyPart::Term(t) => (String::from_utf8(t.clone()).expect("utf-8"), *c),
            other => panic!("not a term: {other:?}"),
        })
        .collect()
}

fn longs(r: &CompositeResult) -> Vec<(i64, u64)> {
    r.buckets
        .iter()
        .map(|(k, c)| match k[0] {
            KeyPart::Long(v) => (v, *c),
            ref other => panic!("not a long: {other:?}"),
        })
        .collect()
}

// AC-1
#[test]
fn a_keyword_source_lists_terms_in_order_and_pages_with_after() {
    let shard = shard("cp-kw", DOCS);
    let src = r#""sources":[{"t":{"terms":{"field":"tag"}}}]"#;
    let first = composite(&shard, &format!(r#"{{"size":3,{src}}}"#));
    assert_eq!(
        terms(&first),
        [
            ("t0".to_owned(), 13),
            ("t1".to_owned(), 13),
            ("t10".to_owned(), 12)
        ]
    );
    let next = composite(
        &shard,
        &format!(r#"{{"size":3,{src},"after":{{"t":"t10"}}}}"#),
    );
    assert_eq!(
        terms(&next),
        [
            ("t11".to_owned(), 12),
            ("t12".to_owned(), 12),
            ("t13".to_owned(), 12)
        ]
    );
    let desc = composite(
        &shard,
        r#"{"size":3,"sources":[{"t":{"terms":{"field":"tag","order":"desc"}}}]}"#,
    );
    assert_eq!(
        terms(&desc),
        [
            ("t9".to_owned(), 12),
            ("t8".to_owned(), 12),
            ("t7".to_owned(), 13)
        ]
    );
}

// AC-3
#[test]
fn paging_with_after_visits_every_bucket_once_in_the_unpaged_order() {
    let shard = shard("cp-page", DOCS);
    let all = terms(&composite(
        &shard,
        r#"{"size":100,"sources":[{"t":{"terms":{"field":"tag"}}}]}"#,
    ));
    assert_eq!(all.len(), 16);
    let mut seen = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let tail = after
            .as_ref()
            .map_or(String::new(), |a| format!(r#","after":{{"t":"{a}"}}"#));
        let page = composite(
            &shard,
            &format!(r#"{{"size":5,"sources":[{{"t":{{"terms":{{"field":"tag"}}}}}}]{tail}}}"#),
        );
        let got = terms(&page);
        let Some(last) = got.last() else { break };
        after = Some(last.0.clone());
        seen.extend(got);
    }
    assert_eq!(seen, all);
}

#[test]
fn a_long_source_keys_by_the_value_and_resumes_after_a_number_or_its_text() {
    let shard = shard("cp-long", DOCS);
    let got = composite(
        &shard,
        r#"{"size":3,"sources":[{"n":{"terms":{"field":"n"}}}],"after":{"n":2}}"#,
    );
    assert_eq!(longs(&got), [(3, 1), (4, 1), (5, 1)]);
    let got = composite(
        &shard,
        r#"{"size":2,"sources":[{"n":{"terms":{"field":"n","order":"desc"}}}],"after":{"n":"198"}}"#,
    );
    assert_eq!(longs(&got), [(197, 1), (196, 1)]);
}

// AC-2
#[test]
fn several_sources_key_by_every_value() {
    let shard = shard("cp-multi", DOCS);
    let got = composite(
        &shard,
        r#"{"size":3,"sources":[{"t":{"terms":{"field":"tag"}}},{"h":{"histogram":{"field":"n","interval":100}}}]}"#,
    );
    let keys: Vec<(String, f64, u64)> = got
        .buckets
        .iter()
        .map(|(k, c)| match (&k[0], &k[1]) {
            (KeyPart::Term(t), KeyPart::Double(h)) => {
                (String::from_utf8(t.clone()).expect("utf-8"), *h, *c)
            }
            other => panic!("unexpected key {other:?}"),
        })
        .collect();
    assert_eq!(
        keys,
        [
            ("t0".to_owned(), 0.0, 7),
            ("t0".to_owned(), 100.0, 6),
            ("t1".to_owned(), 0.0, 7)
        ]
    );
    let after = composite(
        &shard,
        r#"{"size":1,"sources":[{"t":{"terms":{"field":"tag"}}},{"h":{"histogram":{"field":"n","interval":100}}}],"after":{"t":"t0","h":0.0}}"#,
    );
    assert_eq!(after.buckets[0].0[1], KeyPart::Double(100.0));
}

// AC-1
#[test]
fn the_buckets_are_those_of_the_querys_matches_and_a_search_writes_them() {
    let shard = shard("cp-query", DOCS);
    let body = r#"{"size":0,"query":{"range":{"n":{"gte":10,"lt":13}}},"aggs":{"c":{"composite":{"sources":[{"t":{"terms":{"field":"tag"}}}]}}}}"#;
    let mut parts = run(&shard, body).expect("searches");
    let Value::Composite(got) = parts.remove(0).value else {
        panic!("not a composite");
    };
    assert_eq!(
        terms(&got),
        [
            ("t10".to_owned(), 1),
            ("t11".to_owned(), 1),
            ("t12".to_owned(), 1)
        ]
    );
    let reduced = reduce_results(&[&got]).expect("reduces");
    let mut out = Vec::new();
    write_composite(&mut out, &reduced).expect("writes");
    assert_eq!(
        String::from_utf8(out).expect("utf-8"),
        r#""after_key":{"t":"t12"},"buckets":[{"key":{"t":"t10"},"doc_count":1},{"key":{"t":"t11"},"doc_count":1},{"key":{"t":"t12"},"doc_count":1}]"#
    );
}

// AC-10
#[test]
fn a_date_histogram_source_keys_by_the_rounded_millis_and_pages_with_after() {
    let shard = shard("cp-date", DOCS);
    let src = r#""sources":[{"d":{"date_histogram":{"field":"n","fixed_interval":"50ms"}}}]"#;
    let all = composite(&shard, &format!(r#"{{{src}}}"#));
    assert_eq!(longs(&all), [(0, 50), (50, 50), (100, 50), (150, 50)]);
    let next = composite(&shard, &format!(r#"{{"size":1,{src},"after":{{"d":50}}}}"#));
    assert_eq!(longs(&next), [(100, 50)]);
    let desc = composite(
        &shard,
        r#"{"size":2,"sources":[{"d":{"date_histogram":{"field":"n","fixed_interval":"50ms","order":"desc"}}}]}"#,
    );
    assert_eq!(longs(&desc), [(150, 50), (100, 50)]);
    let one = composite(
        &shard,
        r#"{"sources":[{"d":{"date_histogram":{"field":"n","calendar_interval":"minute"}}}]}"#,
    );
    assert_eq!(longs(&one), [(0, 200)]);
    let (status, reason) = refused(
        &shard,
        r#"{"sources":[{"d":{"date_histogram":{"field":"tag","fixed_interval":"1d"}}}]}"#,
    );
    assert_eq!(status, 400);
    assert!(
        reason.contains("not supported for aggregation [date_histogram]"),
        "{reason}"
    );
}

// AC-4
#[test]
fn missing_bucket_on_a_field_every_document_has_adds_no_bucket() {
    let shard = shard("cp-missing", DOCS);
    let got = composite(
        &shard,
        r#"{"size":100,"sources":[{"t":{"terms":{"field":"tag","missing_bucket":true,"missing_order":"first"}}}]}"#,
    );
    assert_eq!(got.buckets.len(), 16);
    assert!(got.buckets.iter().all(|(k, _)| k[0] != KeyPart::Missing));
}

#[test]
fn an_unmapped_source_has_no_buckets_unless_it_asks_for_the_missing_one() {
    let shard = shard("cp-unmapped", DOCS);
    let got = composite(
        &shard,
        r#"{"sources":[{"t":{"terms":{"field":"tag"}}},{"u":{"terms":{"field":"nope"}}}]}"#,
    );
    assert!(got.buckets.is_empty());
    let (status, reason) = refused(
        &shard,
        r#"{"sources":[{"u":{"terms":{"field":"nope","missing_bucket":true}}}]}"#,
    );
    assert_eq!(status, 400);
    assert!(
        reason.contains("is not supported by this node yet"),
        "{reason}"
    );
}

// AC-3
#[test]
fn an_after_opensearch_refuses_is_refused() {
    let shard = shard("cp-after", DOCS);
    let two = r#""sources":[{"a":{"terms":{"field":"tag"}}},{"b":{"terms":{"field":"n"}}}]"#;
    let cases = [
        (
            format!(r#"{{{two},"after":{{"a":"t1"}}}}"#),
            "[after] has 1 value(s) but [sources] has 2",
        ),
        (
            format!(r#"{{{two},"after":{{"a":"t1","c":1}}}}"#),
            "Missing value for [after.b]",
        ),
        (
            format!(r#"{{{two},"after":{{"a":"t1","b":null}}}}"#),
            "Invalid value for [after.b], expected comparable, got [null]",
        ),
        (
            format!(r#"{{{two},"after":{{"a":"t1","b":"x"}}}}"#),
            "For input string: \"x\"",
        ),
    ];
    for (body, want) in cases {
        let (status, reason) = refused(&shard, &body);
        assert_eq!(status, 400, "{body}");
        assert!(reason.contains(want), "{body}: {reason}");
    }
}

// AC-5
#[test]
fn a_source_the_shard_cannot_read_fails_it() {
    let shard = shard("cp-bad", DOCS);
    let (status, reason) = refused(
        &shard,
        r#"{"sources":[{"a":{"histogram":{"field":"tag","interval":2}}}]}"#,
    );
    assert_eq!(status, 400);
    assert!(
        reason.contains("not supported for aggregation [histogram]"),
        "{reason}"
    );
    let (status, reason) = refused(&shard, r#"{"sources":[{"a":{"terms":{"field":"x0"}}}]}"#);
    assert_eq!(status, 400);
    assert!(reason.contains("Text fields are not optimised"), "{reason}");
}

// AC-6
#[test]
fn a_size_over_the_bucket_limit_fails_as_too_many_buckets() {
    let shard = shard("cp-limit", 20);
    let (status, reason) = refused(
        &shard,
        r#"{"size":70000,"sources":[{"a":{"terms":{"field":"tag"}}}]}"#,
    );
    assert_eq!(status, 503);
    assert!(
        reason.contains("Trying to create too many buckets"),
        "{reason}"
    );
}

// AC-8
#[test]
fn a_composite_reserves_its_keys_and_ordinals_with_the_hits() {
    let shard = shard("cp-reserve", DOCS);
    let bytes = |sources: &str, size: usize| {
        let body = format!(
            r#"{{"size":0,"aggs":{{"c":{{"composite":{{"size":{size},"sources":[{sources}]}}}}}}}}"#
        );
        let source = crate::dsl::parse_chunks([body.as_bytes()]).expect("parses");
        crate::search::hits_bytes(&source, &shard)
    };
    let keyword = r#"{"a":{"terms":{"field":"tag"}}}"#;
    let long = r#"{"a":{"terms":{"field":"n"}}}"#;
    let agg = |sources: &str, size: usize| {
        let body = format!(
            r#"{{"aggs":{{"c":{{"composite":{{"size":{size},"sources":[{sources}]}}}}}}}}"#
        );
        let source = crate::dsl::parse_chunks([body.as_bytes()]).expect("parses");
        source.aggs.into_iter().next().expect("one")
    };
    // The keys: exactly `state_bytes`, beside a long source of the same size.
    let spec = |sources: &str, size: usize| agg(sources, size).composite.expect("spec");
    let keys = super::composite_collect::state_bytes(&spec(long, 10));
    assert_eq!(keys, 10 * (48 + 96) * 3);
    assert_eq!(
        super::composite_collect::reserved(&shard, &agg(long, 10)),
        0
    );
    // The ordinals of a keyword source: the dictionary's terms and the
    // kept keys' terms, on top of the keys.
    let ordinals = super::composite_collect::reserved(&shard, &agg(keyword, 10));
    assert!(ordinals >= 16 * 16 + 10 * 3 * 2, "{ordinals}");
    assert_eq!(
        bytes(keyword, 10) - bytes(long, 10),
        ordinals,
        "the search reserves the ordinals"
    );
    // The most a request may keep.
    let most = bytes(long, crate::aggs::bucket::BUCKET_LIMIT);
    assert!(most > bytes(long, 10) + 65_000 * 144 * 3 - 10 * 144 * 3 - 1);
}
