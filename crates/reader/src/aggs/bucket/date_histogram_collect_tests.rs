//! `date_histogram` run on a real index of four segments: its field `n` is
//! a long whose values are 0 to 199, read as epoch milliseconds.
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

use super::date_histogram::{DateHistogramResult, reduce_results};
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::partial::{Format, Partial, Value};
use crate::dsl::parse_chunks;
use crate::search::{ShardError, search};
use crate::{OpenShard, open_shard};

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

fn dates(shard: &OpenShard<Emulator>, body: &str) -> (Format, DateHistogramResult) {
    let mut parts = run(shard, body).expect("searches");
    let part = parts.remove(0);
    match part.value {
        Value::DateHistogram(h) => (part.format, *h),
        other => panic!("not a date histogram: {other:?}"),
    }
}

// AC-2
#[test]
fn documents_count_in_the_interval_of_their_value_across_segments() {
    let shard = shard("dh-all");
    let (format, got) = dates(
        &shard,
        r#"{"size":0,"aggs":{"h":{"date_histogram":{"field":"n","fixed_interval":"50ms"}}}}"#,
    );
    assert_eq!(format, Format::Raw);
    assert_eq!(got.buckets, [(0, 50), (50, 50), (100, 50), (150, 50)]);
    let (_, got) = dates(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"n","fixed_interval":"30ms","offset":"10ms"}}}}"#,
    );
    // Values 0 to 9 round to -20.
    assert_eq!(got.buckets[0], (-20, 10));
    assert_eq!(got.buckets[1], (10, 30));
    assert_eq!(got.buckets.iter().map(|b| b.1).sum::<u64>(), DOCS as u64);
}

#[test]
fn a_calendar_interval_counts_every_document_once() {
    let shard = shard("dh-cal");
    let (_, got) = dates(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"n","calendar_interval":"second"}}}}"#,
    );
    assert_eq!(got.buckets, [(0, 200)]);
    let (_, got) = dates(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"n","calendar_interval":"day","time_zone":"+01:00"}}}}"#,
    );
    // Values 0 to 199 ms are 01:00 local on 1970-01-01, whose midnight is an
    // hour before the epoch.
    assert_eq!(got.buckets, [(-3_600_000, 200)]);
}

// AC-8
#[test]
fn a_region_zones_days_start_at_its_local_midnight() {
    let shard = shard("dh-region");
    let (_, got) = dates(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"n","calendar_interval":"day","time_zone":"Asia/Tokyo"}}}}"#,
    );
    // Values 0 to 199 ms are 09:00 on 1970-01-01 in Tokyo (+09:00).
    assert_eq!(got.buckets, [(-32_400_000, 200)]);
    // Kolkata is at +05:30: the hour of 05:30 local starts half an hour before the epoch.
    let (_, got) = dates(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"n","calendar_interval":"hour","time_zone":"Asia/Kolkata"}}}}"#,
    );
    assert_eq!(got.buckets, [(-1_800_000, 200)]);
}

#[test]
fn the_buckets_are_those_of_the_querys_matches() {
    let shard = shard("dh-query");
    let (_, got) = dates(
        &shard,
        r#"{"query":{"range":{"n":{"gte":10,"lt":23}}},"aggs":{"h":{"date_histogram":{"field":"n","fixed_interval":"10ms"}}}}"#,
    );
    assert_eq!(got.buckets, [(10, 10), (20, 3)]);
}

#[test]
fn hard_bounds_leave_out_the_buckets_outside() {
    let shard = shard("dh-hard");
    let (_, got) = dates(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"n","fixed_interval":"50ms",
            "hard_bounds":{"min":50,"max":100}}}}}"#,
    );
    assert_eq!(got.buckets, [(50, 50)]);
}

// AC-3
#[test]
fn an_unmapped_field_or_no_match_has_no_bucket_but_the_bounds_fill() {
    let shard = shard("dh-none");
    let (_, got) = dates(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"nope","fixed_interval":"5ms"}}}}"#,
    );
    assert!(got.buckets.is_empty());
    let (_, got) = dates(
        &shard,
        r#"{"query":{"match_none":{}},"aggs":{"h":{"date_histogram":{"field":"n","fixed_interval":"5ms"}}}}"#,
    );
    assert!(got.buckets.is_empty());
    let (_, got) = dates(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"nope","fixed_interval":"5ms",
            "extended_bounds":{"min":0,"max":10}}}}}"#,
    );
    let refs = [&got];
    assert_eq!(
        reduce_results(&refs, BUCKET_LIMIT).expect("fits").buckets,
        [(0, 0), (5, 0), (10, 0)]
    );
}

#[test]
fn a_keyword_or_text_field_fails_the_shard() {
    let shard = shard("dh-kw");
    let e = run(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"tag","calendar_interval":"day"}}}}"#,
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
        r#"{"aggs":{"h":{"date_histogram":{"field":"x0","calendar_interval":"day"}}}}"#,
    )
    .expect_err("refused");
    assert!(e.failure.reason.contains("Text fields are not optimised"));
}

// AC-7
#[test]
fn more_buckets_than_the_limit_fail_the_shard_at_503() {
    let shard = shard("dh-many");
    let e = run(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"n","fixed_interval":"1ms",
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
fn a_date_histogram_reserves_its_buckets_with_the_hits() {
    use crate::aggs::metric::state_bytes;
    use crate::search::hits_bytes;
    let shard = shard("dh-bytes");
    let source = |body: &str| parse_chunks([body.as_bytes()]).expect("parses");
    let plain = source(r#"{"size":0}"#);
    let one = source(
        r#"{"size":0,"aggs":{"h":{"date_histogram":{"field":"n","calendar_interval":"day"}}}}"#,
    );
    assert_eq!(
        hits_bytes(&one, &shard) - hits_bytes(&plain, &shard),
        state_bytes(&one.aggs)
    );
    assert!(state_bytes(&one.aggs) >= BUCKET_LIMIT * 64);
}
