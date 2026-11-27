//! Queries, sorts and `search_after` over a `scaled_float` field, on a real index.
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
use oss_testkit::index::{Spec, blobs, doc_scaled_long};

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
    for (key, bytes) in blobs(
        "",
        &Spec::new("scaled", DOCS).segments_of(50).with_decimals(),
    ) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "scaled", 0).expect("opens")
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

/// The double a document's long reads as: the long times the factor's inverse.
fn widened(i: usize) -> f64 {
    doc_scaled_long(i) as f64 * (1.0 / oss_testkit::index::SCALING_FACTOR)
}

#[test]
fn queries_on_a_scaled_float_scale_and_round_their_values() {
    let shard = shard();
    // Document 1 holds 0.123, which a factor of 100 indexes as 12.
    assert_eq!(docs(&shard, r#"{"query":{"term":{"s":0.12}}}"#), [1]);
    assert_eq!(docs(&shard, r#"{"query":{"term":{"s":"0.124"}}}"#), [1]);
    assert_eq!(
        docs(&shard, r#"{"query":{"terms":{"s":[0,0.25]}}}"#),
        [0, 2]
    );
    assert_eq!(
        docs(
            &shard,
            r#"{"query":{"range":{"s":{"gte":0.12,"lte":0.37}}}}"#
        ),
        [1, 2, 3]
    );
    // Excluding a bound steps one hundredth in: 13 to 36 holds only document 2 (25).
    assert_eq!(
        docs(&shard, r#"{"query":{"range":{"s":{"gt":0.12,"lt":0.37}}}}"#),
        [2]
    );
    assert_eq!(
        docs(&shard, r#"{"query":{"range":{"s":{"gt":0.4,"lt":0.2}}}}"#),
        [] as [i32; 0]
    );
}

#[test]
fn a_scaled_float_sort_reads_back_doubles_and_resumes_after_a_key() {
    let shard = shard();
    let hits = run(&shard, r#"{"size":2,"sort":[{"s":"asc"}]}"#).expect("searches");
    let got: Vec<(i32, SortValue)> = hits
        .hits
        .iter()
        .map(|h| (h.doc, h.sort[0].clone()))
        .collect();
    assert_eq!(
        got,
        [
            (0, SortValue::Double(0.0)),
            (1, SortValue::Double(widened(1)))
        ]
    );
    let hits = run(
        &shard,
        r#"{"size":2,"sort":[{"s":"asc"}],"search_after":[0.2]}"#,
    )
    .expect("searches");
    assert_eq!(hits.hits.iter().map(|h| h.doc).collect::<Vec<_>>(), [2, 3]);
    let hits = run(&shard, r#"{"size":1,"sort":[{"s":"desc"}]}"#).expect("searches");
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
fn docvalue_fields_scale_back_and_the_fields_api_rounds_the_source() {
    let shard = shard();
    let text = response(
        &shard,
        r#"{"query":{"term":{"s":0.12}},"docvalue_fields":["s"]}"#,
    );
    assert!(
        text.contains(&format!(r#""fields":{{"s":[{:?}]}}"#, widened(1))),
        "{text}"
    );
    // `Math.round(0.123 * 100) / 100`.
    let text = response(
        &shard,
        r#"{"query":{"term":{"s":0.12}},"fields":["s"],"_source":false}"#,
    );
    assert!(text.contains(r#""fields":{"s":[0.12]}"#), "{text}");
}
