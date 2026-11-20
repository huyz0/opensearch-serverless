//! Sub-aggregations of `range`, `filters` and `filter`, run on a real index
//! of four segments.
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

fn filters_subs(value: Value) -> (Vec<(String, u64)>, Vec<Vec<Value>>) {
    match value {
        Value::Filters(f) => (f.buckets, f.subs),
        other => panic!("not a filters: {other:?}"),
    }
}

fn written(parts: &[Partial]) -> String {
    let mut out = Vec::new();
    write_aggregations(&mut out, parts).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

// AC-1
#[test]
fn each_filters_bucket_holds_the_metrics_of_its_documents() {
    let shard = shard("fixed-filters");
    // `n` is each document's own number 0 to 199; `tag` cycles over 16.
    let (buckets, subs) = filters_subs(first(
        &shard,
        r#"{"size":0,"aggs":{"f":{"filters":{"other_bucket":true,"filters":{
            "low":{"range":{"n":{"lt":10}}},"t0":{"term":{"tag":"t0"}}}},
            "aggs":{"lo":{"min":{"field":"n"}},"hi":{"max":{"field":"n"}},
                    "c":{"value_count":{"field":"n"}}}}}}"#,
    ));
    let counts: Vec<(&str, u64)> = buckets.iter().map(|(k, n)| (k.as_str(), *n)).collect();
    assert_eq!(counts, [("low", 10), ("t0", 13), ("_other_", 178)]);
    let want =
        |lo: f64, hi: f64, c: u64| vec![Value::Min(lo), Value::Max(hi), Value::ValueCount(c)];
    assert_eq!(
        subs,
        [
            want(0.0, 9.0, 10),
            want(0.0, 192.0, 13),
            want(10.0, 199.0, 178)
        ]
    );
}

#[test]
fn each_range_bucket_holds_the_metrics_of_its_documents() {
    let shard = shard("fixed-range");
    let Value::Range(r) = first(
        &shard,
        r#"{"aggs":{"r":{"range":{"field":"n","ranges":[{"to":10},{"from":10,"to":60}]},
            "aggs":{"s":{"sum":{"field":"n"}}}}}}"#,
    ) else {
        panic!("not a range");
    };
    assert_eq!(r.sub_meta.len(), 1);
    assert_eq!(r.sub_meta[0].0, "s");
    // 0 + ... + 9 and 10 + ... + 59.
    assert_eq!(r.subs, [vec![Value::Sum(45.0)], vec![Value::Sum(1725.0)]]);
}

#[test]
fn a_filter_holds_the_metrics_of_its_documents() {
    let shard = shard("fixed-filter");
    let parts = run(
        &shard,
        r#"{"aggs":{"f":{"filter":{"range":{"n":{"lt":10}}},"aggs":{"lo":{"max":{"field":"n"}}}}}}"#,
    )
    .expect("searches");
    let (buckets, subs) = filters_subs(parts[0].value.clone());
    assert_eq!(buckets, [("0".to_owned(), 10)]);
    assert_eq!(subs, [vec![Value::Max(9.0)]]);
    // The whole response nests the count and the sub-aggregation under the
    // aggregation's name.
    assert_eq!(
        written(&parts),
        r#","aggregations":{"f":{"doc_count":10,"lo":{"value":9.0}}}"#
    );
}

// AC-2
#[test]
fn a_keyword_terms_under_a_bucket_counts_that_buckets_documents() {
    let shard = shard("fixed-terms");
    let (_, subs) = filters_subs(first(
        &shard,
        r#"{"aggs":{"f":{"filters":{"filters":[{"range":{"n":{"lt":3}}},{"range":{"n":{"gte":198}}}]},
            "aggs":{"t":{"terms":{"field":"tag","size":2}}}}}}"#,
    ));
    let terms = |bucket: usize| match &subs[bucket][0] {
        Value::KeywordTerms(t) => t.buckets.clone(),
        other => panic!("not a terms: {other:?}"),
    };
    assert_eq!(
        terms(0),
        [
            ("t0".to_owned(), 1),
            ("t1".to_owned(), 1),
            ("t2".to_owned(), 1)
        ]
    );
    assert_eq!(terms(1), [("t6".to_owned(), 1), ("t7".to_owned(), 1)]);
}

// AC-3
#[test]
fn a_range_over_an_unmapped_field_gives_each_bucket_the_empty_value() {
    let shard = shard("fixed-unmapped");
    let Value::Range(r) = first(
        &shard,
        r#"{"aggs":{"r":{"range":{"field":"nope","ranges":[{"to":10},{"from":10}]},
            "aggs":{"c":{"value_count":{"field":"n"}}}}}}"#,
    ) else {
        panic!("not a range");
    };
    assert_eq!(r.buckets.iter().map(|b| b.doc_count).sum::<u64>(), 0);
    assert_eq!(
        r.subs,
        [vec![Value::ValueCount(0)], vec![Value::ValueCount(0)]]
    );
}

// AC-4
#[test]
fn a_sub_aggregation_of_a_numeric_terms_under_a_bucket_is_refused() {
    let shard = shard("fixed-numeric-terms");
    let err = run(
        &shard,
        r#"{"aggs":{"f":{"filters":{"filters":[{"match_all":{}}]},
            "aggs":{"t":{"terms":{"field":"n"}}}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(err.status, 400);
}

// AC-7
#[test]
fn the_sub_aggregations_of_a_bucket_reserve_their_states() {
    let shard = shard("fixed-reserved");
    let aggs = |body: &str| parse_chunks([body.as_bytes()]).expect("parses").aggs;
    let plain = aggs(r#"{"aggs":{"f":{"filters":{"filters":[{"match_all":{}}]}}}}"#);
    let with = aggs(
        r#"{"aggs":{"f":{"filters":{"filters":[{"match_all":{}}]},
            "aggs":{"m":{"min":{"field":"n"}}}}}}"#,
    );
    assert_eq!(super::fixed_subs::reserved(&shard, &plain), 0);
    assert!(super::fixed_subs::reserved(&shard, &with) > 0);
}
