//! A numeric `terms` as a parent and as a sub-aggregation, nested under a
//! bucket, run on a real index.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use super::nested_tests::{first, range_of, run, shard, written};
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::partial::Value;
use crate::dsl::parse_chunks;
use crate::open_shard;

fn terms_of(value: &Value) -> &super::terms::TermsResult {
    match value {
        Value::Terms(t) => t,
        other => panic!("not a numeric terms: {other:?}"),
    }
}

const BY_KEY: &str = r#""order":{"_key":"asc"}"#;

// AC-5
#[test]
fn a_numeric_terms_under_a_range_counts_each_buckets_documents() {
    let shard = shard("nested-terms-under-range");
    let outer = first(
        &shard,
        &format!(
            r#"{{"aggs":{{"o":{{"range":{{"field":"n","ranges":[{{"to":100}},{{"from":100}}]}},
            "aggs":{{"t":{{"terms":{{"field":"n","size":2,{BY_KEY}}}}}}}}}}}}}"#
        ),
    );
    let keys: Vec<Vec<(i64, u64)>> = range_of(&outer)
        .subs
        .iter()
        .map(|list| terms_of(&list[0]).buckets.clone())
        .collect();
    assert_eq!(keys, [vec![(0, 1), (1, 1)], vec![(100, 1), (101, 1)]]);
}

#[test]
fn a_range_under_a_numeric_terms_counts_each_buckets_documents() {
    let shard = shard("nested-range-under-terms");
    let outer = first(
        &shard,
        &format!(
            r#"{{"aggs":{{"t":{{"terms":{{"field":"n","size":2,{BY_KEY}}},
            "aggs":{{"r":{{"range":{{"field":"n","ranges":[{{"to":1}},{{"from":1}}]}}}}}}}}}}}}"#
        ),
    );
    let outer = terms_of(&outer);
    assert_eq!(outer.buckets, [(0, 1), (1, 1)]);
    let inner: Vec<Vec<u64>> = outer
        .subs
        .lists
        .iter()
        .map(|list| {
            range_of(&list[0])
                .buckets
                .iter()
                .map(|b| b.doc_count)
                .collect()
        })
        .collect();
    assert_eq!(inner, [vec![1, 0], vec![0, 1]]);
}

#[test]
fn a_numeric_terms_under_a_numeric_terms_pairs_each_parent_with_its_values() {
    let shard = shard("nested-terms-under-terms");
    let outer = first(
        &shard,
        &format!(
            r#"{{"aggs":{{"t":{{"terms":{{"field":"n","size":2,{BY_KEY}}},
            "aggs":{{"u":{{"terms":{{"field":"n","size":5,{BY_KEY}}}}}}}}}}}}}"#
        ),
    );
    let inner: Vec<Vec<(i64, u64)>> = terms_of(&outer)
        .subs
        .lists
        .iter()
        .map(|list| terms_of(&list[0]).buckets.clone())
        .collect();
    assert_eq!(inner, [vec![(0, 1)], vec![(1, 1)]]);
}

#[test]
fn a_nested_numeric_terms_reduces_across_shards() {
    let a = shard("nested-terms-reduce-a");
    let b = shard("nested-terms-reduce-b");
    let body = format!(
        r#"{{"aggs":{{"o":{{"range":{{"field":"n","ranges":[{{"to":100}}]}},
        "aggs":{{"t":{{"terms":{{"field":"n","size":1,{BY_KEY}}}}}}}}}}}}}"#
    );
    let parts = [first(&a, &body), first(&b, &body)];
    let merged = crate::aggs::partial::reduce_total(&parts)
        .expect("within the limit")
        .expect("reduces");
    assert_eq!(terms_of(&range_of(&merged).subs[0][0]).buckets, [(0, 2)]);
}

#[test]
fn a_nested_numeric_terms_is_written_under_its_bucket() {
    let shard = shard("nested-terms-written");
    let parts = run(
        &shard,
        &format!(
            r#"{{"aggs":{{"o":{{"range":{{"field":"n","keyed":true,"ranges":[{{"key":"lo","to":100}}]}},
            "aggs":{{"t":{{"terms":{{"field":"n","size":1,{BY_KEY}}}}}}}}}}}}}"#
        ),
    )
    .expect("searches");
    assert_eq!(
        written(&parts),
        r#","aggregations":{"o":{"buckets":{"lo":{"to":100.0,"doc_count":100,"t":{"doc_count_error_upper_bound":0,"sum_other_doc_count":99,"buckets":[{"key":0,"doc_count":1}]}}}}}"#
    );
}

// AC-5
#[test]
fn a_nested_numeric_terms_past_the_bucket_limit_fails_instead_of_running() {
    let docs = BUCKET_LIMIT + 5_000;
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::Transport, 64 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(256 << 20, 1), buffers);
    for (key, bytes) in blobs("", &Spec::new("nested-over-limit", docs).segments_of(docs)) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    let big = open_shard(Arc::new(store), cache, "", "nested-over-limit", 0).expect("opens");
    // Every document has its own `n`: one (range bucket, value) pair each.
    let err = run(
        &big,
        r#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"from":0}]},
            "aggs":{"t":{"terms":{"field":"n","size":1}}}}}}"#,
    )
    .expect_err("too many buckets");
    assert_eq!(err.failure.kind, "too_many_buckets_exception");
}

// AC-3
#[test]
fn a_search_reserves_for_a_nested_numeric_terms() {
    let shard = shard("nested-terms-reserved");
    let reserved = |inner: &str| {
        let body = format!(
            r#"{{"aggs":{{"o":{{"range":{{"field":"n","ranges":[{{"to":100}},{{"from":100}}]}},
                "aggs":{{"i":{inner}}}}}}}}}"#
        );
        let aggs = parse_chunks([body.as_bytes()]).expect("parses").aggs;
        super::fixed_subs::reserved(&shard, &aggs)
    };
    let scalar = reserved(r#"{"sum":{"field":"n"}}"#);
    let terms = reserved(r#"{"terms":{"field":"n"}}"#);
    assert!(terms > scalar, "{terms} <= {scalar}");
}

// AC-3
#[test]
fn a_single_valued_level_reserves_for_the_fields_values_not_their_product_with_its_parents() {
    let shard = shard("nested-reserved-tight");
    let aggs = parse_chunks([
        br#"{"aggs":{"o":{"range":{"field":"n","ranges":[{"to":100},{"from":100}]},
        "aggs":{"t":{"terms":{"field":"n","size":3},
            "aggs":{"d":{"date_histogram":{"field":"n","fixed_interval":"100ms"}}}}}}}}"#
            .as_slice(),
    ])
    .expect("parses")
    .aggs;
    // 200 values: each is in one range bucket, one terms bucket and one
    // date bucket, so no level can hold more than a few hundred buckets,
    // not the bucket limit.
    let bytes = super::fixed_subs::reserved(&shard, &aggs);
    assert!(bytes < 1 << 20, "{bytes}");
}

#[test]
fn a_range_parent_multiplies_a_data_keyed_levels_buckets_by_its_width() {
    let shard = shard("nested-reserved-fanout");
    let reserved = |ranges: &str| {
        let body = format!(
            r#"{{"aggs":{{"o":{{"range":{{"field":"n","ranges":{ranges}}},
                "aggs":{{"t":{{"terms":{{"field":"n"}}}}}}}}}}}}"#
        );
        let aggs = parse_chunks([body.as_bytes()]).expect("parses").aggs;
        super::fixed_subs::reserved(&shard, &aggs)
    };
    // Overlapping ranges put a document in each, so more ranges, more buckets.
    let two = reserved(r#"[{"to":100},{"to":150}]"#);
    let four = reserved(r#"[{"to":100},{"to":150},{"to":160},{"to":170}]"#);
    assert!(four > two, "{four} <= {two}");
}
