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

fn shard(name: &str, docs: usize) -> OpenShard<Emulator> {
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

fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Vec<Partial>, ShardError> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
}

fn terms(shard: &OpenShard<Emulator>, body: &str) -> KeywordTermsResult {
    let mut parts = run(shard, body).expect("searches");
    match parts.remove(0).value {
        Value::KeywordTerms(t) => *t,
        other => panic!("not a keyword terms: {other:?}"),
    }
}

fn keys(r: &KeywordTermsResult) -> Vec<(&str, u64)> {
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

#[test]
fn a_zero_min_doc_count_is_refused_on_a_keyword_field() {
    let shard = shard("kw-zero", 20);
    let e = run(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","min_doc_count":0}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(e.status, 400);
    assert!(
        e.failure
            .reason
            .contains("is not supported by this node yet"),
        "{:?}",
        e.failure
    );
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
