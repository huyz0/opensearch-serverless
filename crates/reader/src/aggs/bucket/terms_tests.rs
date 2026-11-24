//! `terms` parsed, reduced and written out.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::terms::{TermsOrder, TermsResult, TermsSpec, reduce_results};
use super::terms_render::write_terms;
use crate::aggs::metric::MetricAgg;
use crate::aggs::partial::Format;
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
        keyword: Default::default(),
        sub_order: None,
        show_error: false,
        shard_min_doc_count: 0,
    }
}

fn shard(spec: &TermsSpec, other: u64, buckets: &[(i64, u64)]) -> TermsResult {
    TermsResult {
        spec: spec.clone(),
        other_doc_count: other,
        doc_count_error: 0,
        buckets: buckets.to_vec(),
        ..TermsResult::default()
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

// An order by a path is a sub-aggregation's: the shard checks the path.
#[test]
fn an_order_by_a_path_is_a_sub_aggregations() {
    let (_, s) = spec_of(r#"{"field":"n","order":{"m.max":"asc"}}"#);
    let order = s.sub_order.expect("by a sub-aggregation");
    assert_eq!((order.path.as_str(), order.ascending), ("m.max", true));
    let (_, s) = spec_of(r#"{"field":"n","order":{"_key":"desc"}}"#);
    assert!(s.sub_order.is_none());
    // The shard size is suggested, as for a count order.
    let (_, s) = spec_of(r#"{"field":"n","size":4,"order":{"m":"desc"}}"#);
    assert_eq!(s.shard_size, 16);
    let body = r#"{"aggs":{"t":{"terms":{"field":"n","order":{"":"desc"}}}}}"#;
    let e = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert_eq!(e.reason, "Invalid path []");
    assert_eq!(e.kind, "illegal_argument_exception");
    let r = reason(r#"{"field":"n","order":{"m[":"desc"}}"#);
    assert!(r.contains("Invalid path element [m[] in path [m[]"), "{r}");
}

#[test]
fn unserved_options_are_refused_as_not_supported_yet() {
    for key in [
        r#""missing":0"#,
        r#""missing":true"#,
        r#""script":{"source":"1"}"#,
        r#""value_type":"long""#,
        r#""format":"yyyy""#,
        r#""execution_hint":"map""#,
        r#""collect_mode":"depth_first""#,
        r#""order":[{"_count":"asc"},{"_key":"asc"}]"#,
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
    // A metric sub-aggregation parses (the shard refuses it under a numeric
    // field); one of another kind is refused here.
    let body =
        r#"{"aggs":{"t":{"terms":{"field":"n"},"aggs":{"m":{"percentiles":{"field":"n"}}}}}}"#;
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

#[test]
fn the_reduce_dispatch_reaches_terms() {
    use crate::aggs::partial::{Value, reduce};
    let s = spec(2, 13, 1, TermsOrder::CountDesc);
    let part = |buckets: &[(i64, u64)]| Value::Terms(Box::new(shard(&s, 0, buckets)));
    let Some(Value::Terms(t)) = reduce(&[part(&[(1, 2)]), part(&[(1, 3), (2, 1)])]) else {
        panic!("not reduced");
    };
    assert_eq!(t.buckets, [(1, 5), (2, 1)]);
    // A terms beside a value of another kind is not a reduce.
    assert!(reduce(&[part(&[(1, 2)]), Value::Min(1.0)]).is_none());
}

// AC-6
#[test]
fn a_bucket_writes_the_error_of_the_shards_that_lack_it() {
    let s = TermsSpec {
        show_error: true,
        shard_min_doc_count: 0,
        ..spec(2, 2, 1, TermsOrder::CountDesc)
    };
    // Both shards are full: their errors are their last counts, 4 and 5.
    let a = shard(&s, 0, &[(1, 10), (2, 4)]);
    let b = shard(&s, 0, &[(2, 6), (3, 5)]);
    let r = reduced(&[a.clone(), b]);
    assert_eq!(r.buckets, [(1, 10), (2, 10)]);
    assert_eq!(r.doc_count_error, 9);
    // Key 1 is missing from `b` (5), key 2 is in both (0).
    assert_eq!(r.bucket_errors, [5, 0]);
    let text = written(Format::Raw, &r);
    assert!(
        text.contains(r#"{"key":1,"doc_count":10,"doc_count_error_upper_bound":5}"#),
        "{text}"
    );
    // One shard holds every bucket: no error.
    let one = reduced(&[a]);
    assert_eq!(one.bucket_errors, [0, 0]);
    // Not asked for: no key, as before.
    let plain = written(
        Format::Raw,
        &shard(&spec(2, 2, 1, TermsOrder::CountDesc), 0, &[(1, 10)]),
    );
    assert!(
        !plain.contains("doc_count_error_upper_bound\":0}"),
        "{plain}"
    );
}

#[test]
fn an_unbounded_shard_makes_every_bucket_error_unbounded() {
    let s = TermsSpec {
        show_error: true,
        shard_min_doc_count: 0,
        ..spec(2, 2, 1, TermsOrder::CountAsc)
    };
    let a = shard(&s, 0, &[(1, 1), (2, 2)]);
    let b = shard(&s, 0, &[(3, 1), (4, 2)]);
    let r = reduced(&[a, b]);
    assert_eq!(r.doc_count_error, -1);
    assert!(r.bucket_errors.iter().all(|e| *e == -1), "{r:?}");
}

#[test]
fn show_term_doc_count_error_parses_as_a_boolean() {
    let (_, s) = spec_of(r#"{"field":"n","show_term_doc_count_error":true}"#);
    assert!(s.show_error);
    let (_, s) = spec_of(r#"{"field":"n","show_term_doc_count_error":false}"#);
    assert!(!s.show_error);
}

// AC-1
#[test]
fn shard_min_doc_count_is_capped_at_min_doc_count_and_defaults_to_zero() {
    let s = |body: &str| spec_of(body).1.shard_min_doc_count;
    assert_eq!(s(r#"{"field":"n"}"#), 0);
    assert_eq!(s(r#"{"field":"n","shard_min_doc_count":3}"#), 1);
    assert_eq!(
        s(r#"{"field":"n","min_doc_count":5,"shard_min_doc_count":3}"#),
        3
    );
    assert_eq!(
        s(r#"{"field":"n","min_doc_count":2,"shard_min_doc_count":"7"}"#),
        2
    );
    assert_eq!(
        s(r#"{"field":"n","min_doc_count":0,"shard_min_doc_count":4}"#),
        0
    );
}

// AC-1
#[test]
fn a_negative_shard_min_doc_count_fails_as_opensearch_fails_it() {
    let r = reason(r#"{"field":"n","shard_min_doc_count":-1}"#);
    assert!(
        r.contains("[shardMinDocCount] must be greater than or equal to 0. Found [-1] in [t]"),
        "{r}"
    );
}

// AC-2
#[test]
fn a_shard_min_doc_count_that_bites_needs_the_count_descending_order() {
    for order in [
        r#"{"_count":"asc"}"#,
        r#"{"_key":"asc"}"#,
        r#"{"_key":"desc"}"#,
    ] {
        let r = reason(&format!(
            r#"{{"field":"n","min_doc_count":3,"shard_min_doc_count":3,"order":{order}}}"#
        ));
        assert!(
            r.contains("is not supported by this node yet"),
            "{order}: {r}"
        );
    }
    // Capped to 0 by `min_doc_count: 0`, it bites nothing and any order serves.
    let s = spec_of(
        r#"{"field":"n","min_doc_count":0,"shard_min_doc_count":3,"order":{"_key":"asc"}}"#,
    );
    assert_eq!(s.1.shard_min_doc_count, 0);
}

// AC-3
#[test]
fn a_shard_drops_buckets_under_its_minimum_and_counts_their_documents_as_others() {
    let mut s = spec(10, 25, 1, TermsOrder::CountDesc);
    s.shard_min_doc_count = 3;
    let mut r = shard(&s, 4, &[(1, 5), (2, 2), (3, 3), (4, 1)]);
    r.subs.lists = vec![vec![], vec![], vec![], vec![]];
    r.apply_shard_min();
    assert_eq!(r.buckets, vec![(1, 5), (3, 3)]);
    assert_eq!(r.other_doc_count, 4 + 2 + 1);
    assert_eq!(r.subs.lists.len(), 2);
    let mut none = shard(&spec(10, 25, 1, TermsOrder::CountDesc), 0, &[(1, 1)]);
    none.apply_shard_min();
    assert_eq!(none.buckets, vec![(1, 1)]);
    assert_eq!(none.other_doc_count, 0);
}
