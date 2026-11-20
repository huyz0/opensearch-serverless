//! `filters` run on a real index of four segments.
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

use crate::aggs::partial::{Partial, Value};
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

fn buckets(shard: &OpenShard<Emulator>, body: &str) -> Vec<(String, u64)> {
    let mut parts = run(shard, body).expect("searches");
    match parts.remove(0).value {
        Value::Filters(r) => r.buckets,
        other => panic!("not a filters: {other:?}"),
    }
}

fn pairs(v: &[(&str, u64)]) -> Vec<(String, u64)> {
    v.iter().map(|(k, n)| ((*k).to_owned(), *n)).collect()
}

// AC-2
#[test]
fn each_filter_counts_the_documents_it_matches_and_other_the_rest() {
    let shard = shard("filters-all", DOCS);
    // `n` is each document's own number 0 to 199; `tag` cycles over 16.
    let got = buckets(
        &shard,
        r#"{"size":0,"aggs":{"f":{"filters":{"other_bucket":true,"filters":{
            "low":{"range":{"n":{"lt":10}}},
            "t0":{"term":{"tag":"t0"}},
            "all":{"match_all":{}}}}}}}"#,
    );
    // Keys sort: all, low, t0; 200 documents, 10 below 10, 200/16 rounded up.
    assert_eq!(
        got,
        pairs(&[("all", 200), ("low", 10), ("t0", 13), ("_other_", 0)])
    );
}

#[test]
fn a_document_counts_in_every_filter_it_matches_and_other_is_what_none_match() {
    let shard = shard("filters-overlap", DOCS);
    let got = buckets(
        &shard,
        r#"{"aggs":{"f":{"filters":{"other_bucket_key":"rest","filters":[
            {"range":{"n":{"lt":100}}},{"range":{"n":{"lt":50}}}]}}}}"#,
    );
    assert_eq!(got, pairs(&[("0", 100), ("1", 50), ("rest", 100)]));
}

#[test]
fn the_counts_are_those_of_the_querys_matches() {
    let shard = shard("filters-query", DOCS);
    let got = buckets(
        &shard,
        r#"{"query":{"range":{"n":{"gte":20,"lt":40}}},"aggs":{"f":{"filters":{
            "filters":[{"range":{"n":{"lt":30}}},{"range":{"n":{"gte":30}}}]}}}}"#,
    );
    assert_eq!(got, pairs(&[("0", 10), ("1", 10)]));
}

// AC-3
#[test]
fn a_query_matching_nothing_counts_zero_and_a_filter_on_an_unmapped_field_matches_none() {
    let shard = shard("filters-none", 20);
    let got = buckets(
        &shard,
        r#"{"query":{"match_none":{}},"aggs":{"f":{"filters":{"other_bucket":true,
            "filters":[{"match_all":{}}]}}}}"#,
    );
    assert_eq!(got, pairs(&[("0", 0), ("_other_", 0)]));
    let got = buckets(
        &shard,
        r#"{"aggs":{"f":{"filters":{"other_bucket":true,"filters":[{"term":{"nope":"x"}}]}}}}"#,
    );
    assert_eq!(got, pairs(&[("0", 0), ("_other_", 20)]));
}

#[test]
fn a_filter_the_shard_cannot_build_fails_the_search_as_a_query_does() {
    let shard = shard("filters-bad", 20);
    let e = run(
        &shard,
        r#"{"aggs":{"f":{"filters":{"filters":[{"term":{"n":"abc"}}]}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(e.status, 400);
}

// AC-7
#[test]
fn more_buckets_than_the_bucket_limit_fail_the_shard() {
    use crate::aggs::bucket::BUCKET_LIMIT;
    let shard = shard("filters-many", 20);
    let wide = |n: usize, other: bool| {
        let mut source = parse_chunks([
            br#"{"aggs":{"f":{"filters":{"filters":[{"match_all":{}}]}}}}"#.as_slice(),
        ])
        .expect("parses");
        let spec = source.aggs[0].filters.as_mut().expect("a filters");
        spec.filters = vec![(String::new(), spec.filters[0].1.clone()); n];
        spec.other_key = other.then(|| "o".to_owned());
        search(&shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
    };
    let e = wide(BUCKET_LIMIT, true).expect_err("refused");
    assert_eq!(e.status, 503);
    assert_eq!(e.failure.kind, "too_many_buckets_exception");
    assert_eq!(
        wide(BUCKET_LIMIT + 1, false).expect_err("refused").status,
        503
    );
    assert!(wide(BUCKET_LIMIT, false).is_ok());
}

#[test]
fn a_filters_reserves_its_buckets_with_the_hits() {
    use crate::aggs::metric::state_bytes;
    let source = |body: &str| parse_chunks([body.as_bytes()]).expect("parses");
    let one = source(r#"{"aggs":{"f":{"filters":{"filters":[{"match_all":{}}]}}}}"#);
    let three = source(
        r#"{"aggs":{"f":{"filters":{"filters":[{"match_all":{}},{"match_all":{}},{"match_all":{}}]}}}}"#,
    );
    let per = std::mem::size_of::<(String, u64)>() + 8;
    // Two more buckets, each with a one-digit key held twice.
    assert_eq!(
        state_bytes(&three.aggs) - state_bytes(&one.aggs),
        2 * per + 4
    );
    let other = source(
        r#"{"aggs":{"f":{"filters":{"other_bucket_key":"abcd","filters":[{"match_all":{}}]}}}}"#,
    );
    assert_eq!(state_bytes(&other.aggs) - state_bytes(&one.aggs), per + 8);
}
