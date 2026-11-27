//! `missing` run on a real index of four segments.
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

use crate::aggs::partial::Value;
use crate::dsl::parse_chunks;
use crate::search::search;
use crate::{OpenShard, open_shard};

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

fn count(shard: &OpenShard<Emulator>, body: &str) -> (u64, Vec<Value>) {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let mut parts = search(shard, "idx", &source, &Analyzer::standard(None))
        .expect("searches")
        .aggregations;
    let Value::Filters(r) = parts.remove(0).value else {
        panic!("not a filter");
    };
    assert!(r.single);
    (r.buckets[0].1, r.subs.into_iter().flatten().collect())
}

// AC-2
#[test]
fn documents_without_the_field_count_and_an_unmapped_field_counts_every_match() {
    let shard = shard("missing-all", 200);
    let got = |q: &str, field: &str| {
        count(
            &shard,
            &format!(r#"{{"size":0,{q}"aggs":{{"m":{{"missing":{{"field":"{field}"}}}}}}}}"#),
        )
        .0
    };
    // Every document has `n` and `tag`.
    assert_eq!(got("", "n"), 0);
    assert_eq!(got("", "tag"), 0);
    assert_eq!(got("", "nope"), 200);
    assert_eq!(got(r#""query":{"range":{"n":{"lt":10}}},"#, "nope"), 10);
    assert_eq!(got(r#""query":{"match_none":{}},"#, "nope"), 0);
}

// AC-4
#[test]
fn a_sub_aggregation_runs_over_the_missing_documents_only() {
    let shard = shard("missing-subs", 200);
    let (n, subs) = count(
        &shard,
        r#"{"aggs":{"m":{"missing":{"field":"nope"},"aggs":{"f":{"filter":{"range":{"n":{"lt":10}}}}}}}}"#,
    );
    assert_eq!(n, 200);
    assert_eq!(subs.len(), 1);
    let (n, subs) = count(
        &shard,
        r#"{"aggs":{"m":{"missing":{"field":"n"},"aggs":{"f":{"filter":{"match_all":{}}}}}}}"#,
    );
    assert_eq!(n, 0);
    let Value::Filters(inner) = &subs[0] else {
        panic!("not a filter");
    };
    assert_eq!(inner.buckets[0].1, 0);
}

// AC-4
#[test]
fn a_missing_runs_as_a_sub_aggregation_over_its_parents_documents() {
    let shard = shard("missing-under", 200);
    let (n, subs) = count(
        &shard,
        r#"{"aggs":{"f":{"filter":{"range":{"n":{"lt":10}}},"aggs":{"m":{"missing":{"field":"nope"}}}}}}"#,
    );
    assert_eq!(n, 10);
    let Value::Filters(inner) = &subs[0] else {
        panic!("not a filter");
    };
    assert_eq!(inner.buckets[0].1, 10);
}
