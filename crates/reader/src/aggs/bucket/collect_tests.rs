//! `terms` run on a real index of four segments.
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

use crate::aggs::bucket::terms::{TermsResult, reduce_results};
use crate::aggs::metric::{Partial, Value};
use crate::dsl::parse_chunks;
use crate::search::{ShardError, search};
use crate::{OpenShard, open_shard};

const DOCS: usize = 200;

fn shard(name: &str, docs: usize) -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", &Spec::new(name, docs).segments_of(50).with_texts(1)) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", name, 0).expect("opens")
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Vec<Partial>, ShardError> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
}

fn terms(shard: &OpenShard<Emulator>, body: &str) -> TermsResult {
    let mut parts = run(shard, body).expect("searches");
    match parts.remove(0).value {
        Value::Terms(t) => *t,
        other => panic!("not a terms: {other:?}"),
    }
}

// AC-2
#[test]
fn every_value_is_a_bucket_ordered_by_count_then_key_and_cut_at_size() {
    let shard = shard("terms-all", DOCS);
    // Each `n` is one document's: every count is 1, so the keys ascend.
    let got = terms(
        &shard,
        r#"{"size":0,"aggs":{"t":{"terms":{"field":"n","size":3}}}}"#,
    );
    // The shard keeps `shard_size` buckets and counts the rest as other.
    assert_eq!(got.spec.shard_size, 14);
    assert_eq!(got.buckets.len(), 14);
    assert_eq!(got.other_doc_count, (DOCS - 14) as u64);
    // The reduce keeps `size`.
    let got = reduce_results(&[&got]);
    assert_eq!(got.buckets, [(0, 1), (1, 1), (2, 1)]);
    assert_eq!(got.other_doc_count, (DOCS - 3) as u64);
}

#[test]
fn key_orders_list_as_opensearch_lists_them() {
    let shard = shard("terms-key", DOCS);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"n","size":3,"order":{"_key":"desc"}}}}}"#,
    );
    assert_eq!(got.buckets, [(199, 1), (198, 1), (197, 1)]);
    assert_eq!(got.other_doc_count, (DOCS - 3) as u64);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"n","size":3,"order":{"_count":"asc"}}}}}"#,
    );
    assert_eq!(
        got.buckets,
        [
            (0, 1),
            (1, 1),
            (2, 1),
            (3, 1),
            (4, 1),
            (5, 1),
            (6, 1),
            (7, 1),
            (8, 1),
            (9, 1),
            (10, 1),
            (11, 1),
            (12, 1),
            (13, 1)
        ]
    );
}

#[test]
fn the_buckets_are_those_of_the_querys_matches() {
    let shard = shard("terms-query", DOCS);
    let got = terms(
        &shard,
        r#"{"query":{"range":{"n":{"gte":10,"lt":13}}},"aggs":{"t":{"terms":{"field":"n"}}}}"#,
    );
    assert_eq!(got.buckets, [(10, 1), (11, 1), (12, 1)]);
    assert_eq!(got.other_doc_count, 0);
}

// AC-3
#[test]
fn min_doc_count_is_the_reduces_not_the_shards() {
    let shard = shard("terms-min", 20);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"n","min_doc_count":2}}}}"#,
    );
    // The shard holds what it counted; the reduce applies the minimum.
    assert_eq!(got.spec.min_doc_count, 2);
    assert_eq!(got.buckets.len(), 20);
}

#[test]
fn an_unmapped_field_or_no_match_has_no_buckets() {
    let shard = shard("terms-none", 20);
    let got = terms(&shard, r#"{"aggs":{"t":{"terms":{"field":"nope"}}}}"#);
    assert_eq!((got.buckets.len(), got.other_doc_count), (0, 0));
    let got = terms(
        &shard,
        r#"{"query":{"match_none":{}},"aggs":{"t":{"terms":{"field":"n"}}}}"#,
    );
    assert_eq!((got.buckets.len(), got.other_doc_count), (0, 0));
}

#[test]
fn a_keyword_or_text_field_fails_the_shard() {
    let shard = shard("terms-kw", 20);
    let e = run(&shard, r#"{"aggs":{"t":{"terms":{"field":"tag"}}}}"#).expect_err("refused");
    assert_eq!(e.status, 400);
    assert!(
        e.failure
            .reason
            .contains("is not supported by this node yet"),
        "{:?}",
        e.failure
    );
    let e = run(&shard, r#"{"aggs":{"t":{"terms":{"field":"x0"}}}}"#).expect_err("refused");
    assert!(
        e.failure.reason.contains("Text fields are not optimised"),
        "{:?}",
        e.failure
    );
}

// AC-7
#[test]
fn more_distinct_values_than_the_bucket_limit_fail_the_shard() {
    use crate::aggs::bucket::BUCKET_LIMIT;
    let shard = shard("terms-many", BUCKET_LIMIT + 1);
    let e = run(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"n","shard_size":70000}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(e.status, 503);
    assert_eq!(e.failure.kind, "too_many_buckets_exception");
    assert!(
        e.failure
            .reason
            .contains("Must be less than or equal to: [65535]"),
        "{:?}",
        e.failure
    );
}

#[test]
fn a_terms_reserves_its_buckets_and_counts_with_the_hits() {
    use crate::aggs::metric::state_bytes;
    use crate::search::{TERMS_BYTES_PER_VALUE, hits_bytes};
    let shard = shard("terms-bytes", 20);
    let source = |body: &str| parse_chunks([body.as_bytes()]).expect("parses");
    let plain = source(r#"{"size":0}"#);
    let one = source(r#"{"size":0,"aggs":{"t":{"terms":{"field":"n","size":1}}}}"#);
    let many = source(r#"{"size":0,"aggs":{"t":{"terms":{"field":"n","size":1000}}}}"#);
    // One distinct value a document of the shard, and the kept buckets.
    assert_eq!(
        hits_bytes(&one, &shard) - hits_bytes(&plain, &shard),
        state_bytes(&one.aggs) + 20 * TERMS_BYTES_PER_VALUE
    );
    assert!(state_bytes(&many.aggs) >= state_bytes(&one.aggs) + (1_500 + 10 - 11) * 16);
}
