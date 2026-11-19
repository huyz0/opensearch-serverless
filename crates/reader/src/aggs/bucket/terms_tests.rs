//! `terms` parsed, reduced and written out.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::terms::{TermsOrder, TermsResult, TermsSpec, reduce_results};
use super::terms_render::write_terms;
use crate::aggs::metric::{Format, MetricAgg};
use crate::dsl::parse_chunks;

fn aggs(body: &str) -> Vec<MetricAgg> {
    parse_chunks([body.as_bytes()]).expect("parses").aggs
}

fn spec_of(terms_body: &str) -> (String, TermsSpec) {
    let body = format!(r#"{{"aggs":{{"t":{{"terms":{terms_body}}}}}}}"#);
    let agg = aggs(&body).remove(0);
    (agg.field, *agg.terms.expect("a terms"))
}

fn reason(terms_body: &str) -> String {
    let body = format!(r#"{{"aggs":{{"t":{{"terms":{terms_body}}}}}}}"#);
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

fn spec(size: usize, shard_size: usize, min: u64, order: TermsOrder) -> TermsSpec {
    TermsSpec {
        size,
        shard_size,
        min_doc_count: min,
        order,
    }
}

fn shard(spec: &TermsSpec, other: u64, buckets: &[(i64, u64)]) -> TermsResult {
    TermsResult {
        spec: spec.clone(),
        other_doc_count: other,
        doc_count_error: 0,
        buckets: buckets.to_vec(),
    }
}

fn reduced(parts: &[TermsResult]) -> TermsResult {
    let refs: Vec<&TermsResult> = parts.iter().collect();
    reduce_results(&refs)
}

fn written(format: Format, result: &TermsResult) -> String {
    let mut out = Vec::new();
    write_terms(&mut out, format, result).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

// AC-1
#[test]
fn defaults_are_opensearchs() {
    let (field, s) = spec_of(r#"{"field":"n"}"#);
    assert_eq!(field, "n");
    assert_eq!(s, spec(10, 25, 1, TermsOrder::CountDesc));
}

#[test]
fn shard_size_follows_size_unless_asked_or_ordered_by_key() {
    assert_eq!(spec_of(r#"{"field":"n","size":3}"#).1.shard_size, 14);
    assert_eq!(spec_of(r#"{"field":"n","size":5}"#).1.shard_size, 17);
    assert_eq!(spec_of(r#"{"field":"n","size":1}"#).1.shard_size, 11);
    // Asked for, and below size: raised to size.
    assert_eq!(
        spec_of(r#"{"field":"n","size":5,"shard_size":2}"#)
            .1
            .shard_size,
        5
    );
    assert_eq!(
        spec_of(r#"{"field":"n","size":5,"shard_size":50}"#)
            .1
            .shard_size,
        50
    );
    // A key order does not suggest a larger shard size.
    assert_eq!(
        spec_of(r#"{"field":"n","size":5,"order":{"_key":"asc"}}"#)
            .1
            .shard_size,
        5
    );
}

#[test]
fn orders_parse() {
    let order = |o: &str| spec_of(&format!(r#"{{"field":"n","order":{o}}}"#)).1.order;
    assert_eq!(order(r#"{"_count":"desc"}"#), TermsOrder::CountDesc);
    assert_eq!(order(r#"{"_count":"asc"}"#), TermsOrder::CountAsc);
    assert_eq!(order(r#"{"_key":"asc"}"#), TermsOrder::KeyAsc);
    assert_eq!(order(r#"{"_key":"desc"}"#), TermsOrder::KeyDesc);
}

#[test]
fn min_doc_count_parses() {
    assert_eq!(
        spec_of(r#"{"field":"n","min_doc_count":0}"#)
            .1
            .min_doc_count,
        0
    );
    assert_eq!(
        spec_of(r#"{"field":"n","min_doc_count":7}"#)
            .1
            .min_doc_count,
        7
    );
}

#[test]
fn bad_options_fail_as_opensearch_fails_them() {
    let r = reason(r#"{"field":"n","size":0}"#);
    assert!(r.contains("[terms] failed to parse field [size]"), "{r}");
    assert!(
        r.contains("[size] must be greater than 0. Found [0] in [t]"),
        "{r}"
    );
    let r = reason(r#"{"field":"n","shard_size":0}"#);
    assert!(
        r.contains("[shardSize] must be greater than 0. Found [0] in [t]"),
        "{r}"
    );
    let r = reason(r#"{"field":"n","min_doc_count":-1}"#);
    assert!(
        r.contains("[minDocCount] must be greater than or equal to 0. Found [-1] in [t]"),
        "{r}"
    );
    let r = reason(r#"{"field":"n","nope":1}"#);
    assert!(r.contains("[terms] unknown field [nope]"), "{r}");
    let r = reason(r#"{"size":3}"#);
    assert!(
        r.contains("Required one of fields [field, script], but none were specified."),
        "{r}"
    );
    let r = reason(r#"{"field":3}"#);
    assert!(
        r.contains("[terms] field doesn't support values of type: VALUE_NUMBER"),
        "{r}"
    );
}

#[test]
fn unserved_options_are_refused_as_not_supported_yet() {
    for key in [
        r#""include":"a.*""#,
        r#""exclude":"a.*""#,
        r#""missing":0"#,
        r#""script":{"source":"1"}"#,
        r#""value_type":"long""#,
        r#""format":"yyyy""#,
        r#""execution_hint":"map""#,
        r#""collect_mode":"depth_first""#,
        r#""show_term_doc_count_error":true"#,
        r#""shard_min_doc_count":2"#,
        r#""order":[{"_count":"asc"},{"_key":"asc"}]"#,
        r#""order":{"sub":"asc"}"#,
    ] {
        let r = reason(&format!(r#"{{"field":"n",{key}}}"#));
        assert!(
            r.contains("is not supported by this node yet"),
            "{key}: {r}"
        );
    }
}

#[test]
fn sub_aggregations_are_refused_as_not_supported_yet() {
    let body = r#"{"aggs":{"t":{"terms":{"field":"n"},"aggs":{"m":{"max":{"field":"n"}}}}}}"#;
    let r = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure()
        .reason;
    assert!(r.contains("is not supported by this node yet"), "{r}");
}

// AC-5
#[test]
fn equal_keys_add_and_the_buckets_are_cut_at_size() {
    let s = spec(2, 13, 1, TermsOrder::CountDesc);
    let a = shard(&s, 0, &[(1, 5), (2, 3), (3, 1)]);
    let b = shard(&s, 1, &[(2, 4), (3, 4), (4, 2)]);
    let got = reduced(&[a, b]);
    // 1:5, 2:7, 3:5, 4:2: by count then key, two kept; the cut hold 5 + 2
    // documents, and the shards' own 1 stays.
    assert_eq!(got.buckets, [(2, 7), (1, 5)]);
    assert_eq!(got.other_doc_count, 8);
    // Neither shard was full, so neither may have cut a bucket.
    assert_eq!(got.doc_count_error, 0);
}

#[test]
fn a_full_shard_may_have_cut_what_its_last_bucket_counts() {
    let s = spec(2, 2, 1, TermsOrder::CountDesc);
    let a = shard(&s, 4, &[(1, 5), (2, 3)]);
    let b = shard(&s, 0, &[(2, 4), (3, 4)]);
    assert_eq!(reduced(&[a, b]).doc_count_error, 3 + 4);
}

#[test]
fn one_shard_has_no_error_even_when_full() {
    let s = spec(2, 2, 1, TermsOrder::CountDesc);
    let got = reduced(&[shard(&s, 9, &[(1, 5), (2, 3)])]);
    assert_eq!(got.doc_count_error, 0);
    assert_eq!(got.other_doc_count, 9);
}

#[test]
fn an_ascending_count_is_unbounded_and_a_key_order_is_exact() {
    let asc = spec(2, 2, 1, TermsOrder::CountAsc);
    let got = reduced(&[
        shard(&asc, 0, &[(1, 1), (2, 3)]),
        shard(&asc, 0, &[(2, 1), (3, 3)]),
    ]);
    assert_eq!(got.doc_count_error, -1);
    let key = spec(2, 2, 1, TermsOrder::KeyAsc);
    let got = reduced(&[
        shard(&key, 0, &[(1, 1), (2, 3)]),
        shard(&key, 0, &[(2, 1), (3, 3)]),
    ]);
    assert_eq!(got.doc_count_error, 0);
    assert_eq!(got.buckets, [(1, 1), (2, 4)]);
    assert_eq!(got.other_doc_count, 3);
}

#[test]
fn min_doc_count_drops_buckets_without_counting_them_as_other() {
    let s = spec(10, 25, 6, TermsOrder::CountDesc);
    let a = shard(&s, 0, &[(1, 5), (2, 3)]);
    let b = shard(&s, 1, &[(2, 4), (3, 4)]);
    let got = reduced(&[a, b]);
    assert_eq!(got.buckets, [(2, 7)]);
    assert_eq!(got.other_doc_count, 1);
}

#[test]
fn key_descending_lists_keys_descending() {
    let s = spec(3, 3, 1, TermsOrder::KeyDesc);
    let a = shard(&s, 0, &[(9, 1), (4, 1)]);
    let b = shard(&s, 0, &[(7, 2), (4, 5), (1, 1)]);
    let got = reduced(&[a, b]);
    assert_eq!(got.buckets, [(9, 1), (7, 2), (4, 6)]);
    assert_eq!(got.other_doc_count, 1);
}

#[test]
fn count_ties_list_by_key_ascending() {
    let s = spec(10, 25, 1, TermsOrder::CountDesc);
    let got = reduced(&[shard(&s, 0, &[(1, 2), (5, 2), (9, 7)])]);
    assert_eq!(got.buckets, [(9, 7), (1, 2), (5, 2)]);
}

// AC-6, AC-4
#[test]
fn a_result_is_written_as_opensearch_writes_it() {
    let s = spec(10, 25, 1, TermsOrder::CountDesc);
    let r = shard(&s, 3, &[(1, 5), (-2, 3)]);
    assert_eq!(
        written(Format::Raw, &r),
        r#""doc_count_error_upper_bound":0,"sum_other_doc_count":3,"buckets":[{"key":1,"doc_count":5},{"key":-2,"doc_count":3}]"#
    );
    let empty = shard(&s, 0, &[]);
    assert_eq!(
        written(Format::Raw, &empty),
        r#""doc_count_error_upper_bound":0,"sum_other_doc_count":0,"buckets":[]"#
    );
}

#[test]
fn a_date_key_is_also_written_as_text() {
    let s = spec(10, 25, 1, TermsOrder::CountDesc);
    let r = shard(&s, 0, &[(0, 1), (86_400_000, 1)]);
    assert_eq!(
        written(Format::Date, &r),
        concat!(
            r#""doc_count_error_upper_bound":0,"sum_other_doc_count":0,"buckets":["#,
            r#"{"key":0,"key_as_string":"1970-01-01T00:00:00.000Z","doc_count":1},"#,
            r#"{"key":86400000,"key_as_string":"1970-01-02T00:00:00.000Z","doc_count":1}]"#
        )
    );
}

#[test]
fn a_single_shard_result_is_written_reduced() {
    let s = spec(1, 11, 1, TermsOrder::CountDesc);
    let r = shard(&s, 0, &[(1, 2), (2, 5), (3, 1)]);
    assert_eq!(
        written(Format::Raw, &r),
        r#""doc_count_error_upper_bound":0,"sum_other_doc_count":3,"buckets":[{"key":2,"doc_count":5}]"#
    );
}
