//! `histogram` run on a real index of four segments.
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

use super::histogram::{HistogramResult, reduce_results};
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::partial::{Partial, Value};
use crate::dsl::parse_chunks;
use crate::search::{ShardError, search};
use crate::{OpenShard, open_shard};

fn reduced(parts: &[HistogramResult]) -> HistogramResult {
    let refs: Vec<&HistogramResult> = parts.iter().collect();
    reduce_results(&refs, BUCKET_LIMIT).expect("within the limit")
}

const DOCS: usize = 200;

fn shard(name: &str) -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", &Spec::new(name, DOCS).segments_of(50).with_texts(1)) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", name, 0).expect("opens")
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Vec<Partial>, ShardError> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
}

fn histogram(shard: &OpenShard<Emulator>, body: &str) -> HistogramResult {
    let mut parts = run(shard, body).expect("searches");
    match parts.remove(0).value {
        Value::Histogram(h) => *h,
        other => panic!("not a histogram: {other:?}"),
    }
}

// AC-2
#[test]
fn documents_count_in_the_interval_of_their_value_across_segments() {
    let shard = shard("hist-all");
    let got = histogram(
        &shard,
        r#"{"size":0,"aggs":{"h":{"histogram":{"field":"n","interval":50}}}}"#,
    );
    assert_eq!(
        got.buckets,
        [(0.0, 50), (50.0, 50), (100.0, 50), (150.0, 50)]
    );
    let got = histogram(
        &shard,
        r#"{"aggs":{"h":{"histogram":{"field":"n","interval":30,"offset":10}}}}"#,
    );
    // Keys are `n * interval + offset`: values 0..9 fall in -20.
    assert_eq!(got.buckets[0], (-20.0, 10));
    assert_eq!(got.buckets[1], (10.0, 30));
    assert_eq!(got.buckets.iter().map(|b| b.1).sum::<u64>(), DOCS as u64);
}

#[test]
fn the_buckets_are_those_of_the_querys_matches() {
    let shard = shard("hist-query");
    let got = histogram(
        &shard,
        r#"{"query":{"range":{"n":{"gte":10,"lt":23}}},"aggs":{"h":{"histogram":{"field":"n","interval":10}}}}"#,
    );
    assert_eq!(got.buckets, [(10.0, 10), (20.0, 3)]);
}

#[test]
fn hard_bounds_leave_out_the_buckets_outside() {
    let shard = shard("hist-hard");
    let got = histogram(
        &shard,
        r#"{"aggs":{"h":{"histogram":{"field":"n","interval":50,
            "hard_bounds":{"min":50,"max":100}}}}}"#,
    );
    assert_eq!(got.buckets, [(50.0, 50), (100.0, 50)]);
}

// AC-3
#[test]
fn an_unmapped_field_or_no_match_has_no_bucket_but_the_bounds_fill() {
    let shard = shard("hist-none");
    let got = histogram(
        &shard,
        r#"{"aggs":{"h":{"histogram":{"field":"nope","interval":5}}}}"#,
    );
    assert!(got.buckets.is_empty());
    let got = histogram(
        &shard,
        r#"{"query":{"match_none":{}},"aggs":{"h":{"histogram":{"field":"n","interval":5}}}}"#,
    );
    assert!(got.buckets.is_empty());
    // The reduce, not the shard, fills the extended bounds.
    let got = histogram(
        &shard,
        r#"{"aggs":{"h":{"histogram":{"field":"nope","interval":5,
            "extended_bounds":{"min":0,"max":10}}}}}"#,
    );
    assert_eq!(reduced(&[got]).buckets, [(0.0, 0), (5.0, 0), (10.0, 0)]);
}

#[test]
fn a_missing_interval_fails_the_shard() {
    let shard = shard("hist-interval");
    let e = run(&shard, r#"{"aggs":{"h":{"histogram":{"field":"n"}}}}"#).expect_err("refused");
    assert_eq!(e.status, 400);
    assert_eq!(e.failure.kind, "illegal_argument_exception");
    assert!(
        e.failure
            .reason
            .contains("interval must be positive, got: 0.0")
    );
}

#[test]
fn a_keyword_or_text_field_fails_the_shard() {
    let shard = shard("hist-kw");
    let e = run(
        &shard,
        r#"{"aggs":{"h":{"histogram":{"field":"tag","interval":1}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(e.status, 400);
    assert!(
        e.failure.reason.contains("Field [tag] of type [keyword]"),
        "{:?}",
        e.failure
    );
    let e = run(
        &shard,
        r#"{"aggs":{"h":{"histogram":{"field":"x0","interval":1}}}}"#,
    )
    .expect_err("refused");
    assert!(e.failure.reason.contains("Text fields are not optimised"));
}

// AC-7
#[test]
fn more_buckets_than_the_limit_fail_the_shard_at_503() {
    let shard = shard("hist-many");
    let e = run(
        &shard,
        r#"{"aggs":{"h":{"histogram":{"field":"n","interval":1,
            "extended_bounds":{"min":0,"max":70000}}}}}"#,
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
fn a_histogram_reserves_its_buckets_with_the_hits() {
    use crate::aggs::metric::state_bytes;
    use crate::search::hits_bytes;
    let shard = shard("hist-bytes");
    let source = |body: &str| parse_chunks([body.as_bytes()]).expect("parses");
    let plain = source(r#"{"size":0}"#);
    let one = source(r#"{"size":0,"aggs":{"h":{"histogram":{"field":"n","interval":1}}}}"#);
    assert_eq!(
        hits_bytes(&one, &shard) - hits_bytes(&plain, &shard),
        state_bytes(&one.aggs)
    );
    assert!(state_bytes(&one.aggs) >= BUCKET_LIMIT * 64);
}
