//! A keyword `terms` reduced and written out.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::keyword_terms::{KeywordTermsResult, reduce, reduce_results, write_keyword_terms};
use super::terms::{TermsOrder, TermsResult, TermsSpec};
use crate::aggs::partial::Value;

fn spec(size: usize, shard_size: usize, min: u64, order: TermsOrder) -> TermsSpec {
    TermsSpec {
        size,
        shard_size,
        min_doc_count: min,
        order,
    }
}

fn shard(spec: &TermsSpec, other: u64, buckets: &[(&str, u64)]) -> KeywordTermsResult {
    KeywordTermsResult {
        spec: spec.clone(),
        other_doc_count: other,
        doc_count_error: 0,
        buckets: buckets.iter().map(|&(k, c)| (k.to_owned(), c)).collect(),
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
