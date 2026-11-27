//! Queries, sorts and `search_after` over a `half_float` field, on a real index.
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
use oss_testkit::index::{Spec, blobs, doc_half};

use super::*;
use crate::dsl::parse_chunks;
use crate::{OpenShard, open_shard};

const DOCS: usize = 200;

fn shard() -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", &Spec::new("half", DOCS).segments_of(50).with_decimals()) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "half", 0).expect("opens")
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Hits, ShardError> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None))
}

fn docs(shard: &OpenShard<Emulator>, body: &str) -> Vec<i32> {
    let hits = run(shard, body).expect("searches");
    let mut docs: Vec<i32> = hits.hits.iter().map(|h| h.doc).collect();
    docs.sort_unstable();
    docs
}

/// The half a document holds: its tenth rounded to a half.
fn stored(i: usize) -> f32 {
    use lucene_util::half_float::{half_float_to_sortable_short, sortable_short_to_half_float};
    sortable_short_to_half_float(half_float_to_sortable_short(doc_half(i)))
}

#[test]
fn queries_on_a_half_float_read_the_nearest_half() {
    let shard = shard();
    // Document 1 holds the half nearest 0.1, which the query's 0.1 rounds to as well.
    assert_ne!(stored(1), 0.1);
    assert_eq!(docs(&shard, r#"{"query":{"term":{"h":0.1}}}"#), [1]);
    assert_eq!(
        docs(&shard, r#"{"query":{"terms":{"h":[0,"0.2"]}}}"#),
        [0, 2]
    );
    assert_eq!(
        docs(&shard, r#"{"query":{"range":{"h":{"gte":0.5,"lte":1.0}}}}"#),
        [5, 6, 7, 8, 9, 10]
    );
    // Excluding 0.2 steps one half in, so document 2 is out and 3 is in.
    assert_eq!(
        docs(&shard, r#"{"query":{"range":{"h":{"gt":0.2,"lt":0.4}}}}"#),
        [3]
    );
    assert_eq!(
        docs(&shard, r#"{"query":{"range":{"h":{"gt":0.4,"lt":0.2}}}}"#),
        [] as [i32; 0]
    );
}

#[test]
fn a_value_past_the_half_range_is_refused_as_the_mapper_refuses_it() {
    let shard = shard();
    let reason = run(&shard, r#"{"query":{"term":{"h":70000}}}"#)
        .expect_err("refused")
        .failure
        .reason;
    assert_eq!(
        reason,
        "failed to create query: [half_float] supports only finite values, but got [70000.0]"
    );
}

#[test]
fn a_half_float_sort_reads_back_floats_and_resumes_after_a_key() {
    let shard = shard();
    let hits = run(&shard, r#"{"size":2,"sort":[{"h":"asc"}]}"#).expect("searches");
    let got: Vec<(i32, SortValue)> = hits
        .hits
        .iter()
        .map(|h| (h.doc, h.sort[0].clone()))
        .collect();
    assert_eq!(
        got,
        [(0, SortValue::Float(0.0)), (1, SortValue::Float(stored(1)))]
    );
    let hits = run(
        &shard,
        r#"{"size":2,"sort":[{"h":"asc"}],"search_after":["0.2"]}"#,
    )
    .expect("searches");
    assert_eq!(hits.hits.iter().map(|h| h.doc).collect::<Vec<_>>(), [3, 4]);
    let hits = run(&shard, r#"{"size":1,"sort":[{"h":"desc"}]}"#).expect("searches");
    assert_eq!(hits.hits.first().map(|h| h.doc), Some(199));
}

/// The hit's JSON as the response writes it.
fn response(shard: &OpenShard<Emulator>, body: &str) -> String {
    let hits = run(shard, body).expect("searches");
    let mut out = Vec::new();
    crate::response::write_response(&mut out, shard, "idx", "node", &hits, 3, &mut |_| Ok(()))
        .expect("writes");
    String::from_utf8(out).expect("utf-8")
}

#[test]
fn docvalue_fields_widen_a_half_and_the_fields_api_reads_the_source() {
    let shard = shard();
    let text = response(
        &shard,
        r#"{"query":{"term":{"h":0.1}},"docvalue_fields":["h"]}"#,
    );
    // The stored half, which a double shows in full.
    assert!(
        text.contains(r#""fields":{"h":[0.0999755859375]}"#),
        "{text}"
    );
    let text = response(
        &shard,
        r#"{"query":{"term":{"h":0.1}},"fields":["h"],"_source":false}"#,
    );
    assert!(text.contains(r#""fields":{"h":[0.1]}"#), "{text}");
}
