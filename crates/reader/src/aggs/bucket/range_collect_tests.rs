//! `range` run on a real index of four segments.
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

use crate::aggs::bucket::range::RangeDef;
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

fn counts(shard: &OpenShard<Emulator>, body: &str) -> Vec<u64> {
    let mut parts = run(shard, body).expect("searches");
    match parts.remove(0).value {
        Value::Range(r) => r.buckets.iter().map(|b| b.doc_count).collect(),
        other => panic!("not a range: {other:?}"),
    }
}

// AC-2
#[test]
fn each_range_counts_the_documents_from_inclusive_to_exclusive() {
    let shard = shard("range-all", DOCS);
    // `n` is each document's own number, 0 to 199, over four segments.
    let got = counts(
        &shard,
        r#"{"size":0,"aggs":{"r":{"range":{"field":"n","ranges":[
            {"to":10},{"from":10,"to":60},{"from":150},{"from":55,"to":160}]}}}}"#,
    );
    // Sorted by from, then to: (-inf,10), [10,60), [55,160), [150,inf).
    assert_eq!(got, [10, 50, 105, 50]);
}

#[test]
fn the_counts_are_those_of_the_querys_matches() {
    let shard = shard("range-query", DOCS);
    let got = counts(
        &shard,
        r#"{"query":{"range":{"n":{"gte":20,"lt":40}}},"aggs":{"r":{"range":{"field":"n",
            "ranges":[{"to":30},{"from":30}]}}}}"#,
    );
    assert_eq!(got, [10, 10]);
}

// AC-3
#[test]
fn an_unmapped_field_or_no_match_counts_nothing_in_every_range() {
    let shard = shard("range-none", 20);
    let body = r#"{"aggs":{"r":{"range":{"field":"nope","ranges":[{"to":1},{"from":1}]}}}}"#;
    assert_eq!(counts(&shard, body), [0, 0]);
    let none = r#"{"query":{"match_none":{}},"aggs":{"r":{"range":{"field":"n","ranges":[{"to":1},{"from":1}]}}}}"#;
    assert_eq!(counts(&shard, none), [0, 0]);
}

#[test]
fn a_keyword_or_text_field_fails_the_shard() {
    let shard = shard("range-kw", 20);
    for field in ["tag", "x0"] {
        let body = format!(
            r#"{{"aggs":{{"r":{{"range":{{"field":"{field}","ranges":[{{"to":1}}]}}}}}}}}"#
        );
        let e = run(&shard, &body).expect_err("refused");
        assert_eq!(e.status, 400, "{field}");
    }
    let e = run(
        &shard,
        r#"{"aggs":{"r":{"range":{"field":"tag","ranges":[{"to":1}]}}}}"#,
    )
    .expect_err("refused");
    assert!(
        e.failure
            .reason
            .contains("Field [tag] of type [keyword] is not supported for aggregation [range]"),
        "{:?}",
        e.failure
    );
}

// AC-7
#[test]
fn more_ranges_than_the_bucket_limit_fail_the_shard() {
    use crate::aggs::bucket::BUCKET_LIMIT;
    let shard = shard("range-many", 20);
    // The recording of the body bounds how many ranges parse, so the shard's
    // own bound is reached by widening the parsed request.
    let wide = |n: usize| {
        let mut source = parse_chunks([
            br#"{"aggs":{"r":{"range":{"field":"n","ranges":[{"from":0}]}}}}"#.as_slice(),
        ])
        .expect("parses");
        let spec = source.aggs[0].range.as_mut().expect("a range");
        spec.ranges = (0..n)
            .map(|i| RangeDef {
                key: None,
                from: i as f64,
                to: f64::INFINITY,
            })
            .collect();
        search(&shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
    };
    let e = wide(BUCKET_LIMIT + 1).expect_err("refused");
    assert_eq!(e.status, 503);
    assert_eq!(e.failure.kind, "too_many_buckets_exception");
    let ok = wide(BUCKET_LIMIT).expect("searches");
    assert!(matches!(&ok[0].value, Value::Range(r) if r.buckets.len() == BUCKET_LIMIT));
}

#[test]
fn a_range_reserves_its_buckets_with_the_hits() {
    use crate::aggs::metric::state_bytes;
    use crate::search::hits_bytes;
    let shard = shard("range-bytes", 20);
    let source = |body: &str| parse_chunks([body.as_bytes()]).expect("parses");
    let one = source(r#"{"size":0,"aggs":{"r":{"range":{"field":"n","ranges":[{"to":1}]}}}}"#);
    let three = source(
        r#"{"size":0,"aggs":{"r":{"range":{"field":"n","ranges":[{"to":1},{"to":2},{"to":3}]}}}}"#,
    );
    let size = std::mem::size_of::<crate::aggs::bucket::range::RangeBucket>() + 16 + 8;
    assert_eq!(state_bytes(&three.aggs) - state_bytes(&one.aggs), 2 * size);
    // A key's text is held by the request and by the result.
    let keyed = source(
        r#"{"size":0,"aggs":{"r":{"range":{"field":"n","ranges":[{"to":1,"key":"abcd"}]}}}}"#,
    );
    assert_eq!(state_bytes(&keyed.aggs) - state_bytes(&one.aggs), 8);
    assert!(hits_bytes(&three, &shard) >= state_bytes(&three.aggs));
}
