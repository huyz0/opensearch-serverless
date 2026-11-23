//! A keyword `terms` run on a real index of four segments.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use crate::aggs::bucket::keyword_terms::{KeywordTermsResult, reduce_results};
use crate::aggs::partial::{Partial, Value};
use crate::dsl::parse_chunks;
use crate::search::{ShardError, search};
use crate::{OpenShard, open_shard};

/// Two hundred documents in four segments of fifty: `tag` is `t<i % 16>`,
/// so `t0` to `t7` have 13 documents and `t8` to `t15` have 12.
const DOCS: usize = 200;

pub(super) fn shard(name: &str, docs: usize) -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    let spec = Spec::new(name, docs)
        .segments_of(50)
        .with_texts(1)
        .with_keywords(1);
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", name, 0).expect("opens")
}

pub(super) fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Vec<Partial>, ShardError> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
}

pub(super) fn terms(shard: &OpenShard<Emulator>, body: &str) -> KeywordTermsResult {
    let mut parts = run(shard, body).expect("searches");
    match parts.remove(0).value {
        Value::KeywordTerms(t) => *t,
        other => panic!("not a keyword terms: {other:?}"),
    }
}

pub(super) fn keys(r: &KeywordTermsResult) -> Vec<(&str, u64)> {
    r.buckets.iter().map(|(k, c)| (k.as_str(), *c)).collect()
}

// AC-1
#[test]
fn terms_count_across_segments_ordered_by_count_then_term_bytes() {
    let shard = shard("kw-count", DOCS);
    let got = terms(
        &shard,
        r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag","size":3,"shard_size":16}}}}"#,
    );
    assert_eq!(got.buckets.len(), 16);
    let got = reduce_results(&[&got]);
    // 13 documents each for t0 to t7: ties list by term.
    assert_eq!(keys(&got), [("t0", 13), ("t1", 13), ("t2", 13)]);
    assert_eq!(got.other_doc_count, (DOCS - 39) as u64);
    // The twelves: `t10` sorts before `t8` as bytes.
    let all = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","size":16,"shard_size":16}}}}"#,
    );
    let all = reduce_results(&[&all]);
    let tail: Vec<&str> = keys(&all)[8..].iter().map(|b| b.0).collect();
    assert_eq!(tail, ["t10", "t11", "t12", "t13", "t14", "t15", "t8", "t9"]);
    assert_eq!(all.other_doc_count, 0);
}

// AC-3
#[test]
fn a_keyword_shard_min_doc_count_drops_the_twelves_into_other() {
    let shard = shard("kw-shard-min", DOCS);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","size":16,"shard_size":16,"min_doc_count":13,"shard_min_doc_count":13}}}}"#,
    );
    assert_eq!(got.buckets.len(), 8);
    assert!(got.buckets.iter().all(|b| b.1 == 13));
    assert_eq!(got.other_doc_count, (DOCS - 8 * 13) as u64);
}

#[test]
fn the_shard_keeps_shard_size_buckets_and_counts_the_rest_as_other() {
    let shard = shard("kw-shard", DOCS);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","size":3}}}}"#,
    );
    // `shard_size` 14 of 16 terms: the two smallest are other.
    assert_eq!(got.buckets.len(), 14);
    assert_eq!(got.other_doc_count, 24);
}

#[test]
fn key_and_ascending_count_orders_list_as_opensearch_lists_them() {
    let shard = shard("kw-order", DOCS);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","size":3,"order":{"_key":"desc"}}}}}"#,
    );
    assert_eq!(keys(&got), [("t9", 12), ("t8", 12), ("t7", 13)]);
    assert_eq!(got.other_doc_count, DOCS as u64 - 37);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","size":2,"order":{"_key":"asc"}}}}}"#,
    );
    assert_eq!(keys(&got), [("t0", 13), ("t1", 13)]);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","size":2,"order":{"_count":"asc"}}}}}"#,
    );
    let got = reduce_results(&[&got]);
    assert_eq!(keys(&got), [("t10", 12), ("t11", 12)]);
}

#[test]
fn the_buckets_are_those_of_the_querys_matches() {
    let shard = shard("kw-query", DOCS);
    let got = terms(
        &shard,
        r#"{"query":{"range":{"n":{"gte":10,"lt":13}}},"aggs":{"t":{"terms":{"field":"tag"}}}}"#,
    );
    assert_eq!(keys(&got), [("t10", 1), ("t11", 1), ("t12", 1)]);
    assert_eq!(got.other_doc_count, 0);
    // A second keyword field, seven values cycling.
    let got = terms(&shard, r#"{"aggs":{"t":{"terms":{"field":"k0"}}}}"#);
    assert_eq!(got.buckets.len(), 7);
    assert_eq!(got.buckets.iter().map(|b| b.1).sum::<u64>(), DOCS as u64);
}

// AC-2
#[test]
fn min_doc_count_is_the_reduces_not_the_shards() {
    let shard = shard("kw-min", 20);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","min_doc_count":2}}}}"#,
    );
    assert_eq!(got.spec.min_doc_count, 2);
    // t0 to t3 have two documents of 20, t4 to t15 one: the shard holds all.
    assert_eq!(got.buckets.len(), 16);
    let got = reduce_results(&[&got]);
    assert_eq!(keys(&got), [("t0", 2), ("t1", 2), ("t2", 2), ("t3", 2)]);
    assert_eq!(got.other_doc_count, 0);
}

#[test]
fn an_unmapped_field_or_no_match_has_no_buckets() {
    let shard = shard("kw-none", 20);
    let parts = run(&shard, r#"{"aggs":{"t":{"terms":{"field":"nope"}}}}"#).expect("runs");
    assert!(
        matches!(&parts[0].value, Value::Terms(t) if t.buckets.is_empty() && t.other_doc_count == 0)
    );
    let got = terms(
        &shard,
        r#"{"query":{"match_none":{}},"aggs":{"t":{"terms":{"field":"tag"}}}}"#,
    );
    assert_eq!((got.buckets.len(), got.other_doc_count), (0, 0));
}

// AC-5
#[test]
fn a_keyword_terms_reserves_its_ordinals_and_keys_with_the_hits() {
    use crate::aggs::metric::state_bytes;
    use crate::search::hits_bytes;
    let shard = shard("kw-bytes", DOCS);
    let source = |body: &str| parse_chunks([body.as_bytes()]).expect("parses");
    let plain = source(r#"{"size":0}"#);
    let one = source(r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag","size":1}}}}"#);
    // Four segments of sixteen terms, each at most three bytes: 64 terms,
    // two buffers a segment, and 11 buckets of three key copies and 64 bytes.
    let reserved = 64 * 40 + 4 * 3 * 2 + 11 * (3 * 3 + 64);
    assert_eq!(
        hits_bytes(&one, &shard) - hits_bytes(&plain, &shard),
        state_bytes(&one.aggs) + reserved
    );
    // The keys are bounded by the terms there are.
    let many = source(r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag","size":1000}}}}"#);
    let capped = hits_bytes(&many, &shard) - hits_bytes(&plain, &shard) - state_bytes(&many.aggs);
    assert_eq!(capped, 64 * 40 + 4 * 3 * 2 + 64 * (3 * 3 + 64));
    // A field no segment holds reserves nothing.
    let none = source(r#"{"size":0,"aggs":{"t":{"terms":{"field":"nope","size":1}}}}"#);
    assert_eq!(
        hits_bytes(&none, &shard) - hits_bytes(&plain, &shard),
        state_bytes(&none.aggs) + DOCS * crate::search::TERMS_BYTES_PER_VALUE
    );
}

fn reduced_keys(shard: &OpenShard<Emulator>, body: &str) -> Vec<(String, u64)> {
    let got = terms(shard, body);
    reduce_results(&[&got]).buckets
}

fn pairs(list: &[(&str, u64)]) -> Vec<(String, u64)> {
    list.iter().map(|&(k, c)| (k.to_owned(), c)).collect()
}

// AC-7
#[test]
fn include_and_exclude_regexes_keep_the_accepted_terms() {
    let shard = shard("kw-regex", DOCS);
    let got = reduced_keys(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":"t1.*","size":20,"shard_size":20}}}}"#,
    );
    assert_eq!(
        got,
        pairs(&[
            ("t1", 13),
            ("t10", 12),
            ("t11", 12),
            ("t12", 12),
            ("t13", 12),
            ("t14", 12),
            ("t15", 12)
        ])
    );
    // A pattern matches the whole term: `t1` alone is one term.
    let got = reduced_keys(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":"t1"}}}}"#,
    );
    assert_eq!(got, pairs(&[("t1", 13)]));
    // Exclude runs after include.
    let got = reduced_keys(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":"t1.*","exclude":"t1[0-2]","size":20,"shard_size":20}}}}"#,
    );
    assert_eq!(
        got,
        pairs(&[("t1", 13), ("t13", 12), ("t14", 12), ("t15", 12)])
    );
    // Exclude alone keeps every other term.
    let all = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","exclude":"t1.*","size":20,"shard_size":20}}}}"#,
    );
    assert_eq!(all.buckets.len(), 9);
    assert_eq!(all.other_doc_count, 0);
}

#[test]
fn include_and_exclude_sets_keep_the_listed_terms() {
    let shard = shard("kw-set", DOCS);
    let got = reduced_keys(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":["t9","t3","nope"]}}}}"#,
    );
    assert_eq!(got, pairs(&[("t3", 13), ("t9", 12)]));
    let got = reduced_keys(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":["t9","t3"],"exclude":["t9"]}}}}"#,
    );
    assert_eq!(got, pairs(&[("t3", 13)]));
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","exclude":["t0","t1"],"size":20,"shard_size":20}}}}"#,
    );
    assert_eq!(got.buckets.len(), 14);
}

#[test]
fn a_rejected_terms_documents_are_not_counted_as_other() {
    let shard = shard("kw-other", DOCS);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":"t3"}}}}"#,
    );
    assert_eq!(got.other_doc_count, 0);
}

#[test]
fn a_zero_min_doc_count_lists_the_terms_no_match_holds() {
    let shard = shard("kw-zero-list", DOCS);
    let body = |extra: &str| {
        format!(
            r#"{{"query":{{"range":{{"n":{{"gte":10,"lt":13}}}}}},"aggs":{{"t":{{"terms":{{"field":"tag","min_doc_count":0,"size":20,"shard_size":20{extra}}}}}}}}}"#
        )
    };
    let got = terms(&shard, &body(""));
    assert_eq!(got.buckets.len(), 16);
    let got = reduce_results(&[&got]);
    // The three that matched, then the empty ones by term.
    assert_eq!(
        got.buckets[..4],
        pairs(&[("t10", 1), ("t11", 1), ("t12", 1), ("t0", 0)])[..]
    );
    assert_eq!(got.buckets.iter().map(|b| b.1).sum::<u64>(), 3);
    // Only the accepted terms are empty buckets.
    let got = terms(&shard, &body(r#","include":"t1.*""#));
    let got = reduce_results(&[&got]);
    assert_eq!(
        got.buckets,
        pairs(&[
            ("t10", 1),
            ("t11", 1),
            ("t12", 1),
            ("t1", 0),
            ("t13", 0),
            ("t14", 0),
            ("t15", 0)
        ])
    );
    // No match at all: every term, empty.
    let got = terms(
        &shard,
        r#"{"query":{"match_none":{}},"aggs":{"t":{"terms":{"field":"tag","min_doc_count":0,"size":20,"shard_size":20}}}}"#,
    );
    assert_eq!(got.buckets.len(), 16);
}

#[test]
fn a_zero_min_doc_count_keeps_shard_size_buckets() {
    let shard = shard("kw-zero-cut", DOCS);
    let got = terms(
        &shard,
        r#"{"query":{"match_none":{}},"aggs":{"t":{"terms":{"field":"tag","min_doc_count":0,"size":2,"shard_size":5}}}}"#,
    );
    assert_eq!(got.buckets.len(), 5);
    assert_eq!(got.other_doc_count, 0);
}

#[test]
fn a_missing_term_not_in_the_dictionary_is_a_bucket_among_the_ordinals() {
    let shard = shard("kw-missing", 20);
    // Every document has a value: the term is empty, and listed in order.
    let got = reduced_keys(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"k0","missing":"a0","min_doc_count":0,"size":20,"shard_size":20,"order":{"_key":"asc"}}}}}"#,
    );
    assert_eq!(got[0], ("a0".to_owned(), 0));
    assert_eq!(got.len(), 8);
    // Filters number the inserted term among the ordinals: include only it,
    // and only terms after it.
    let got = reduced_keys(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"k0","missing":"v35","include":"v3.*|v5","min_doc_count":0,"size":20,"shard_size":20,"order":{"_key":"asc"}}}}}"#,
    );
    assert_eq!(
        got.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(),
        ["v3", "v35", "v5"]
    );
    // A missing term the dictionary holds adds nothing.
    let got = reduced_keys(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"k0","missing":"v3","size":20,"shard_size":20}}}}"#,
    );
    assert_eq!(got.len(), 7);
    // A filter over a term the dictionary holds keeps the ordinals as they are.
    let got = reduced_keys(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"k0","missing":"v3","include":"v3|v4","size":20,"shard_size":20,"order":{"_key":"asc"}}}}}"#,
    );
    assert_eq!(
        got.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(),
        ["v3", "v4"]
    );
    // An excluded inserted term is no bucket.
    let got = reduced_keys(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"k0","missing":"v35","exclude":"v35","min_doc_count":0,"size":20,"shard_size":20}}}}"#,
    );
    assert_eq!(got.len(), 7);
}

#[test]
fn a_bad_pattern_fails_the_shard_as_opensearch_fails_it() {
    let shard = shard("kw-bad-regex", 20);
    let e = run(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":"(a"}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(e.status, 400);
    assert_eq!(e.failure.kind, "illegal_argument_exception");
    assert!(e.failure.reason.contains("expected ')'"), "{:?}", e.failure);
    let long = "a".repeat(1001);
    let e = run(
        &shard,
        &format!(r#"{{"aggs":{{"t":{{"terms":{{"field":"tag","exclude":"{long}"}}}}}}}}"#),
    )
    .expect_err("refused");
    assert_eq!(
        e.failure.reason,
        "The length of regex [1001] used in the request has exceeded the allowed maximum of \
         [1000]. This maximum can be set by changing the [index.max_regex_length] index level \
         setting."
    );
}

#[test]
fn keyword_only_options_are_refused_on_a_numeric_field() {
    let shard = shard("kw-numeric", 20);
    for option in [r#""include":"1""#, r#""exclude":["1"]"#, r#""missing":"1""#] {
        let e = run(
            &shard,
            &format!(r#"{{"aggs":{{"t":{{"terms":{{"field":"n",{option}}}}}}}}}"#),
        )
        .expect_err("refused");
        assert_eq!(e.status, 400);
        assert!(
            e.failure
                .reason
                .contains("of the numeric field [n] is not supported by this node yet"),
            "{option}: {:?}",
            e.failure
        );
    }
}

// AC-8
#[test]
fn filters_and_empty_buckets_reserve_their_working_arrays() {
    use crate::aggs::metric::state_bytes;
    use crate::search::hits_bytes;
    let shard = shard("kw-bytes-more", DOCS);
    let source = |body: &str| parse_chunks([body.as_bytes()]).expect("parses");
    let base = source(r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag","size":1}}}}"#);
    let zero =
        source(r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag","size":1,"min_doc_count":0}}}}"#);
    let filtered =
        source(r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag","size":1,"include":"t1.*"}}}}"#);
    let bytes = |s: &crate::dsl::SearchSource| hits_bytes(s, &shard) - state_bytes(&s.aggs);
    // 64 terms: each empty bucket candidate is 32 bytes.
    assert_eq!(bytes(&zero) - bytes(&base), 64 * 32);
    // The accepted bits twice (64 / 8 bytes each, one word more), and the
    // longest term.
    assert_eq!(bytes(&filtered) - bytes(&base), 2 * (64 / 8 + 8) + 3);
}

#[test]
fn a_pattern_too_complex_to_determinize_is_refused() {
    let shard = shard("kw-complex", 20);
    let e = run(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":".*a.{18}"}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(e.status, 400);
    assert!(
        e.failure
            .reason
            .contains("too complex to determinize is not supported by this node yet"),
        "{:?}",
        e.failure
    );
}

#[test]
fn the_pattern_length_limit_counts_utf16_units() {
    let shard = shard("kw-length", 20);
    let body = |pattern: &str| {
        format!(r#"{{"aggs":{{"t":{{"terms":{{"field":"tag","include":"{pattern}"}}}}}}}}"#)
    };
    // 1,000 units is allowed.
    assert!(run(&shard, &body(&"a".repeat(1000))).is_ok());
    // 501 characters outside the basic plane are 1,002 units.
    let astral = "\u{1F600}".repeat(501);
    let e = run(&shard, &body(&astral)).expect_err("refused");
    assert!(e.failure.reason.contains("regex [1002]"), "{:?}", e.failure);
}

#[test]
fn two_aggregations_of_a_field_have_their_own_filters() {
    let shard = shard("kw-two", DOCS);
    let parts = run(
        &shard,
        r#"{"aggs":{"a":{"terms":{"field":"tag","include":"t3"}},"b":{"terms":{"field":"tag","exclude":"t.*"}}}}"#,
    )
    .expect("runs");
    let Value::KeywordTerms(a) = &parts[0].value else {
        panic!("{:?}", parts[0])
    };
    let Value::KeywordTerms(b) = &parts[1].value else {
        panic!("{:?}", parts[1])
    };
    assert_eq!(keys(a), [("t3", 13)]);
    assert!(b.buckets.is_empty());
}
