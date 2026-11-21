//! A keyword `terms` reduced and written out.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::keyword_terms::{KeywordTermsResult, reduce, reduce_results, write_keyword_terms};
use super::terms::{TermsOrder, TermsResult, TermsSpec};
use crate::aggs::partial::{Format, Value};

fn spec(size: usize, shard_size: usize, min: u64, order: TermsOrder) -> TermsSpec {
    TermsSpec {
        size,
        shard_size,
        min_doc_count: min,
        order,
        keyword: Default::default(),
        show_error: false,
    }
}

fn shard(spec: &TermsSpec, other: u64, buckets: &[(&str, u64)]) -> KeywordTermsResult {
    KeywordTermsResult {
        spec: spec.clone(),
        other_doc_count: other,
        doc_count_error: 0,
        buckets: buckets.iter().map(|&(k, c)| (k.to_owned(), c)).collect(),
        bucket_errors: Vec::new(),
        sub_meta: Vec::new(),
        subs: Vec::new(),
    }
}

fn reduced(parts: &[KeywordTermsResult]) -> KeywordTermsResult {
    let refs: Vec<&KeywordTermsResult> = parts.iter().collect();
    reduce_results(&refs)
}

fn keys(r: &KeywordTermsResult) -> Vec<(&str, u64)> {
    r.buckets.iter().map(|(k, c)| (k.as_str(), *c)).collect()
}

fn written(result: &KeywordTermsResult) -> String {
    let mut out = Vec::new();
    write_keyword_terms(&mut out, result).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

// AC-3
#[test]
fn equal_keys_add_and_the_buckets_are_cut_at_size() {
    let s = spec(2, 12, 1, TermsOrder::CountDesc);
    let a = shard(&s, 1, &[("a", 5), ("b", 3), ("c", 1)]);
    let b = shard(&s, 2, &[("b", 4), ("d", 2)]);
    let got = reduced(&[a, b]);
    assert_eq!(keys(&got), [("b", 7), ("a", 5)]);
    // The cut buckets (d: 2, c: 1) join what the shards cut already.
    assert_eq!(got.other_doc_count, 1 + 2 + 2 + 1);
}

#[test]
fn count_ties_list_by_term_bytes_ascending() {
    let s = spec(10, 25, 1, TermsOrder::CountDesc);
    let got = reduced(&[shard(&s, 0, &[("b", 2), ("B", 2), ("a", 2), ("é", 2)])]);
    assert_eq!(keys(&got), [("B", 2), ("a", 2), ("b", 2), ("é", 2)]);
}

#[test]
fn the_other_orders_list_as_opensearch_lists_them() {
    let ascending = spec(3, 3, 1, TermsOrder::CountAsc);
    let got = reduced(&[shard(&ascending, 0, &[("a", 3), ("b", 1), ("c", 2)])]);
    assert_eq!(keys(&got), [("b", 1), ("c", 2), ("a", 3)]);
    let by_key = spec(3, 3, 1, TermsOrder::KeyAsc);
    let got = reduced(&[shard(&by_key, 0, &[("c", 3), ("a", 1), ("b", 2)])]);
    assert_eq!(keys(&got), [("a", 1), ("b", 2), ("c", 3)]);
    let by_key_desc = spec(2, 2, 1, TermsOrder::KeyDesc);
    let got = reduced(&[shard(&by_key_desc, 0, &[("c", 3), ("a", 1), ("b", 2)])]);
    assert_eq!(keys(&got), [("c", 3), ("b", 2)]);
    assert_eq!(got.other_doc_count, 1);
}

#[test]
fn min_doc_count_drops_buckets_without_counting_them_as_other() {
    let s = spec(10, 25, 3, TermsOrder::CountDesc);
    let got = reduced(&[
        shard(&s, 0, &[("a", 2), ("b", 1)]),
        shard(&s, 0, &[("a", 2), ("c", 1)]),
    ]);
    assert_eq!(keys(&got), [("a", 4)]);
    assert_eq!(got.other_doc_count, 0);
}

#[test]
fn the_error_bound_is_the_numeric_terms() {
    let s = spec(2, 2, 1, TermsOrder::CountDesc);
    // One shard: exact, even when full.
    assert_eq!(
        reduced(&[shard(&s, 4, &[("a", 5), ("b", 3)])]).doc_count_error,
        0
    );
    // Two full shards: each may have cut what its last bucket counts.
    let got = reduced(&[
        shard(&s, 4, &[("a", 5), ("b", 3)]),
        shard(&s, 4, &[("c", 6), ("a", 2)]),
    ]);
    assert_eq!(got.doc_count_error, 3 + 2);
    // A shard that returned fewer than `shard_size` buckets cut nothing.
    let got = reduced(&[
        shard(&s, 0, &[("a", 5)]),
        shard(&s, 4, &[("c", 6), ("a", 2)]),
    ]);
    assert_eq!(got.doc_count_error, 2);
    // Ascending counts are unbounded.
    let up = spec(2, 2, 1, TermsOrder::CountAsc);
    let got = reduced(&[
        shard(&up, 0, &[("a", 1), ("b", 3)]),
        shard(&up, 0, &[("c", 1), ("a", 2)]),
    ]);
    assert_eq!(got.doc_count_error, -1);
}

#[test]
fn a_keyword_and_a_numeric_terms_do_not_reduce() {
    let s = spec(10, 25, 1, TermsOrder::CountDesc);
    let keyword = Value::KeywordTerms(Box::new(shard(&s, 0, &[("a", 1)])));
    let numeric = Value::Terms(Box::new(TermsResult {
        spec: s.clone(),
        other_doc_count: 0,
        doc_count_error: 0,
        buckets: vec![(1, 1)],
        ..TermsResult::default()
    }));
    assert!(reduce(&[keyword.clone(), numeric.clone()]).is_none());
    assert!(reduce(&[numeric, keyword.clone()]).is_none());
    assert!(reduce(&[]).is_none());
    assert!(matches!(reduce(&[keyword]), Some(Value::KeywordTerms(_))));
    assert!(
        crate::aggs::partial::reduce(&[Value::KeywordTerms(Box::new(shard(&s, 0, &[])))]).is_some()
    );
}

// AC-4
#[test]
fn a_result_is_written_as_opensearch_writes_it() {
    let s = spec(10, 25, 1, TermsOrder::CountDesc);
    assert_eq!(
        written(&shard(&s, 3, &[("tea", 5), ("coffee", 3)])),
        r#""doc_count_error_upper_bound":0,"sum_other_doc_count":3,"buckets":[{"key":"tea","doc_count":5},{"key":"coffee","doc_count":3}]"#
    );
    assert_eq!(
        written(&shard(&s, 0, &[])),
        r#""doc_count_error_upper_bound":0,"sum_other_doc_count":0,"buckets":[]"#
    );
}

#[test]
fn keys_are_escaped_as_json_strings() {
    let s = spec(10, 25, 1, TermsOrder::CountDesc);
    assert_eq!(
        written(&shard(&s, 0, &[("a\"b\\c\n", 1)])),
        r#""doc_count_error_upper_bound":0,"sum_other_doc_count":0,"buckets":[{"key":"a\"b\\c\n","doc_count":1}]"#
    );
}

#[test]
fn a_single_shard_result_is_written_reduced() {
    // The shard keeps `shard_size` buckets; the response keeps `size`.
    let s = spec(1, 11, 1, TermsOrder::CountDesc);
    assert_eq!(
        written(&shard(&s, 0, &[("a", 5), ("b", 3)])),
        r#""doc_count_error_upper_bound":0,"sum_other_doc_count":3,"buckets":[{"key":"a","doc_count":5}]"#
    );
}

/// A shard result whose buckets each carry a `sum` and a `max`.
fn with_subs(buckets: &[(&str, u64, f64, f64)]) -> KeywordTermsResult {
    let mut r = shard(
        &spec(10, 10, 1, TermsOrder::CountDesc),
        0,
        &buckets.iter().map(|b| (b.0, b.1)).collect::<Vec<_>>(),
    );
    r.sub_meta = vec![("s".to_owned(), Format::Raw), ("m".to_owned(), Format::Raw)];
    r.subs = buckets
        .iter()
        .map(|b| vec![Value::Sum(b.2), Value::Max(b.3)])
        .collect();
    r
}

// AC-11
#[test]
fn equal_keys_reduce_their_sub_aggregations_and_the_cut_buckets_take_theirs() {
    let mut a = with_subs(&[("x", 5, 10.0, 4.0), ("y", 3, 1.0, 9.0)]);
    let mut b = with_subs(&[("x", 4, 20.0, 7.0), ("z", 1, 2.0, 2.0)]);
    a.spec.size = 2;
    b.spec.size = 2;
    let got = reduced(&[a, b]);
    assert_eq!(keys(&got), [("x", 9), ("y", 3)]);
    assert_eq!(
        got.subs,
        [
            vec![Value::Sum(30.0), Value::Max(7.0)],
            vec![Value::Sum(1.0), Value::Max(9.0)]
        ]
    );
    assert_eq!(got.sub_meta.len(), 2);
}

// AC-12
#[test]
fn a_bucket_writes_its_sub_aggregations_after_doc_count() {
    let got = written(&with_subs(&[("x", 5, 10.0, 4.0)]));
    // `s` and `m` list in the order `hash_map_order` gives, which is not
    // the request's.
    let (s, m) = (
        got.find("\"s\":{").expect("s"),
        got.find("\"m\":{").expect("m"),
    );
    // Java's HashMap (16 slots) puts `s` (115) in slot 3 and `m` (109) in 13.
    assert!(s < m, "{got}");
    assert!(got.contains("\"doc_count\":5,"), "{got}");
    assert!(m > got.find("doc_count").expect("count"), "{got}");
    assert!(got.contains("\"s\":{\"value\":10.0}"), "{got}");
    assert!(got.contains("\"m\":{\"value\":4.0}"), "{got}");
    assert!(got.ends_with("}}]"), "{got}");
}

// AC-15
#[test]
fn nested_results_reduce_and_write_inside_their_buckets() {
    let inner = |a: u64, b: u64| {
        let mut r = shard(
            &spec(10, 10, 1, TermsOrder::CountDesc),
            1,
            &[("p", a), ("q", b)],
        );
        r.doc_count_error = 0;
        Value::KeywordTerms(Box::new(r))
    };
    let outer = |count: u64, a: u64, b: u64| {
        let mut r = shard(&spec(10, 10, 1, TermsOrder::CountDesc), 0, &[("x", count)]);
        r.sub_meta = vec![("n".to_owned(), Format::Raw)];
        r.subs = vec![vec![inner(a, b)]];
        r
    };
    let got = reduced(&[outer(5, 2, 3), outer(4, 4, 1)]);
    assert_eq!(keys(&got), [("x", 9)]);
    let Value::KeywordTerms(n) = &got.subs[0][0] else {
        panic!("{:?}", got.subs);
    };
    assert_eq!(keys(n), [("p", 6), ("q", 4)]);
    assert_eq!(n.other_doc_count, 2);
    let out = written(&got);
    assert!(
        out.contains(
            "\"doc_count\":9,\"n\":{\"doc_count_error_upper_bound\":0,\"sum_other_doc_count\":2,\"buckets\":[{\"key\":\"p\",\"doc_count\":6},{\"key\":\"q\",\"doc_count\":4}]}}"
        ),
        "{out}"
    );
}

#[test]
fn a_keyword_bucket_writes_the_error_of_the_shards_that_lack_it() {
    let s = TermsSpec {
        show_error: true,
        ..spec(2, 2, 1, TermsOrder::CountDesc)
    };
    let a = shard(&s, 0, &[("a", 10), ("b", 4)]);
    let b = shard(&s, 0, &[("b", 6), ("c", 5)]);
    let r = reduced(&[a, b]);
    assert_eq!(keys(&r), [("a", 10), ("b", 10)]);
    assert_eq!(r.bucket_errors, [5, 0]);
    let mut out = Vec::new();
    write_keyword_terms(&mut out, &r).expect("writes");
    let text = String::from_utf8(out).expect("utf-8");
    assert!(
        text.contains(r#"{"key":"a","doc_count":10,"doc_count_error_upper_bound":5}"#),
        "{text}"
    );
}
