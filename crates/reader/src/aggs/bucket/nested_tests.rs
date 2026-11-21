//! A bucket aggregation under a bucket, run on a real index of four segments.
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

use crate::aggs::metric::write_aggregations;
use crate::aggs::partial::{Partial, Value};
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

fn first(shard: &OpenShard<Emulator>, body: &str) -> Value {
    run(shard, body).expect("searches").remove(0).value
}

fn written(parts: &[Partial]) -> String {
    let mut out = Vec::new();
    write_aggregations(&mut out, parts).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

fn range_of(value: &Value) -> &super::range::RangeResult {
    match value {
        Value::Range(r) => r,
        other => panic!("not a range: {other:?}"),
    }
}

// AC-1
#[test]
fn a_range_under_a_range_counts_each_buckets_documents() {
    let shard = shard("nested-range");
    // `n` is each document's own number 0 to 199.
    let outer = first(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100},{"from":100}]},
            "aggs":{"i":{"range":{"field":"n","ranges":[{"to":50},{"from":50,"to":150},{"from":150}]}}}}}}"#,
    );
    let outer = range_of(&outer);
    let counts: Vec<u64> = outer.buckets.iter().map(|b| b.doc_count).collect();
    assert_eq!(counts, [100, 100]);
    assert_eq!(outer.sub_meta[0].0, "i");
    let inner: Vec<Vec<u64>> = outer
        .subs
        .iter()
        .map(|list| {
            range_of(&list[0])
                .buckets
                .iter()
                .map(|b| b.doc_count)
                .collect()
        })
        .collect();
    assert_eq!(inner, [vec![50, 50, 0], vec![0, 50, 50]]);
}

#[test]
fn a_metric_under_a_range_under_a_range_reads_the_inner_buckets_documents() {
    let shard = shard("nested-range-metric");
    let outer = first(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100},{"from":100}]},
            "aggs":{"i":{"range":{"field":"n","ranges":[{"to":50},{"from":50}]},
                "aggs":{"s":{"sum":{"field":"n"}}}}}}}}"#,
    );
    let outer = range_of(&outer);
    let sums: Vec<Vec<Value>> = outer
        .subs
        .iter()
        .map(|list| range_of(&list[0]).subs.iter().flatten().cloned().collect())
        .collect();
    // 0..50 and 50..100, then none and 100..200.
    assert_eq!(
        sums,
        [
            vec![Value::Sum(1225.0), Value::Sum(3725.0)],
            vec![Value::Sum(0.0), Value::Sum(14950.0)]
        ]
    );
}

#[test]
fn a_nested_range_is_written_under_its_bucket() {
    let shard = shard("nested-range-written");
    let parts = run(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","keyed":true,"ranges":[{"key":"lo","to":100}]},
            "aggs":{"i":{"range":{"field":"n","keyed":true,"ranges":[{"key":"a","to":50}]}}}}}}"#,
    )
    .expect("searches");
    assert_eq!(
        written(&parts),
        r#","aggregations":{"o":{"buckets":{"lo":{"to":100.0,"doc_count":100,"i":{"buckets":{"a":{"to":50.0,"doc_count":50}}}}}}}"#
    );
}

#[test]
fn a_nested_range_over_an_unmapped_field_has_the_empty_buckets() {
    let shard = shard("nested-range-unmapped");
    let outer = first(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100}]},
            "aggs":{"i":{"range":{"field":"missing","ranges":[{"to":50}]}}}}}}"#,
    );
    let inner = range_of(&range_of(&outer).subs[0][0]);
    assert_eq!(inner.buckets.len(), 1);
    assert_eq!(inner.buckets[0].doc_count, 0);
}

// AC-8
#[test]
fn a_bucket_aggregation_under_a_keyword_terms_is_still_refused() {
    let err = parse_chunks([
        br#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"i":{"range":{"field":"n","ranges":[{"to":5}]}}}}}}"#
            .as_slice(),
    ])
    .expect_err("refused")
    .into_failure();
    assert!(
        err.reason.contains("[range] sub-aggregation"),
        "{}",
        err.reason
    );
}

#[test]
fn a_histogram_under_a_range_has_its_own_buckets_in_each() {
    let shard = shard("nested-histogram");
    let outer = first(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100},{"from":100}]},
            "aggs":{"h":{"histogram":{"field":"n","interval":50},
                "aggs":{"s":{"sum":{"field":"n"}}}}}}}}"#,
    );
    let outer = range_of(&outer);
    type Seen = (Vec<(f64, u64)>, Vec<Vec<Value>>);
    let seen: Vec<Seen> = outer
        .subs
        .iter()
        .map(|list| match &list[0] {
            Value::Histogram(h) => (h.buckets.clone(), h.subs.lists.clone()),
            other => panic!("not a histogram: {other:?}"),
        })
        .collect();
    assert_eq!(
        seen,
        [
            (
                vec![(0.0, 50), (50.0, 50)],
                vec![vec![Value::Sum(1225.0)], vec![Value::Sum(3725.0)]]
            ),
            (
                vec![(100.0, 50), (150.0, 50)],
                vec![vec![Value::Sum(6225.0)], vec![Value::Sum(8725.0)]]
            ),
        ]
    );
}

#[test]
fn a_date_histogram_under_a_range_has_its_own_buckets_in_each() {
    let shard = shard("nested-date-histogram");
    let outer = first(
        &shard,
        r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100},{"from":100}]},
            "aggs":{"h":{"date_histogram":{"field":"n","fixed_interval":"50ms"}}}}}}"#,
    );
    let keys: Vec<Vec<(i64, u64)>> = range_of(&outer)
        .subs
        .iter()
        .map(|list| match &list[0] {
            Value::DateHistogram(h) => h.buckets.clone(),
            other => panic!("not a date histogram: {other:?}"),
        })
        .collect();
    assert_eq!(keys, [vec![(0, 50), (50, 50)], vec![(100, 50), (150, 50)]]);
}

// AC-3
#[test]
fn shards_nested_ranges_add_bucket_for_bucket() {
    let a = shard("nested-reduce-a");
    let b = shard("nested-reduce-b");
    let body = r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100}]},
        "aggs":{"i":{"range":{"field":"n","ranges":[{"to":50},{"from":50}]}}}}}}"#;
    let parts = [first(&a, body), first(&b, body)];
    let merged = crate::aggs::partial::reduce_total(&parts)
        .expect("within the limit")
        .expect("reduces");
    let inner: Vec<u64> = range_of(&range_of(&merged).subs[0][0])
        .buckets
        .iter()
        .map(|b| b.doc_count)
        .collect();
    assert_eq!(inner, [100, 100]);
}

// AC-6
#[test]
fn a_search_reserves_for_the_buckets_of_each_nested_level() {
    let shard = shard("nested-reserved");
    let reserved = |body: &str| {
        let aggs = parse_chunks([body.as_bytes()]).expect("parses").aggs;
        super::fixed_subs::reserved(&shard, &aggs)
    };
    let range_under = |inner: &str| {
        format!(
            r#"{{"aggs":{{"o":{{"range":{{"field":"n","ranges":[{{"to":100}},{{"from":100}}]}},
                "aggs":{{"i":{inner}}}}}}}}}"#
        )
    };
    let scalar = reserved(&range_under(r#"{"sum":{"field":"n"}}"#));
    let ranges = reserved(&range_under(
        r#"{"range":{"field":"n","ranges":[{"to":50},{"from":50}]}}"#,
    ));
    let wider = reserved(&range_under(
        r#"{"range":{"field":"n","ranges":[{"to":10},{"to":20},{"to":30},{"to":40}]}}"#,
    ));
    let histogram = reserved(&range_under(r#"{"histogram":{"field":"n","interval":10}}"#));
    // Two parent buckets of two ranges each: more buckets than a metric's.
    assert!(ranges > scalar, "{ranges} <= {scalar}");
    assert!(wider > ranges, "{wider} <= {ranges}");
    // A histogram can make a bucket for each of the field's values in each.
    assert!(histogram > ranges, "{histogram} <= {ranges}");
}
