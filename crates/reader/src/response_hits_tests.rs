//! The response's hits written from a real index: whether each hit's
//! `_source` and the total are written, and what filtering reserves.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, Rounds, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs, doc_source};

use super::write_response;
use crate::dsl::parse_chunks;
use crate::fetch::source_filter::scratch_bytes;
use crate::search::search;
use crate::{OpenShard, open_shard};

const DOCS: usize = 20;

fn spec() -> Spec {
    Spec::new("hits", DOCS).segments_of(10).padded(40)
}

fn shard() -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", &spec()) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "hits", 0).expect("opens")
}

/// The response body for `body`, and every reservation it made.
fn respond(shard: &OpenShard<Emulator>, body: &str) -> (String, Vec<usize>) {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let hits = search(shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    let mut out = Vec::new();
    let mut grants = Vec::new();
    write_response(&mut out, shard, "idx", "node", &hits, 0, &mut |n| {
        grants.push(n);
        Ok(())
    })
    .expect("writes");
    (String::from_utf8(out).expect("utf-8"), grants)
}

#[test]
fn each_hit_s_source_is_written_whole_filtered_or_not_at_all() {
    let shard = shard();
    let sorted = r#""sort":[{"n":"asc"}],"size":2"#;
    let (whole, _) = respond(&shard, &format!("{{{sorted}}}"));
    let first = doc_source(&spec(), 0);
    assert!(
        whole.contains(&format!(r#""_source":{first},"sort""#)),
        "{whole}"
    );
    let (none, _) = respond(&shard, &format!(r#"{{{sorted},"_source":false}}"#));
    assert!(!none.contains("_source"), "{none}");
    assert!(none.contains(r#""_score":null,"sort":[0]"#), "{none}");
    let (some, _) = respond(&shard, &format!(r#"{{{sorted},"_source":"n"}}"#));
    assert!(some.contains(r#""_source":{"n":0},"sort":[0]"#), "{some}");
    assert!(some.contains(r#""_source":{"n":1},"sort":[1]"#), "{some}");
}

#[test]
fn filtering_reserves_its_scratch_once_for_the_largest_source() {
    let shard = shard();
    let (_, whole) = respond(&shard, r#"{"sort":[{"n":"asc"}]}"#);
    let (_, filtered) = respond(&shard, r#"{"sort":[{"n":"asc"}],"_source":["tag","n"]}"#);
    let largest = (0..10)
        .map(|i| doc_source(&spec(), i).len())
        .max()
        .unwrap_or(0);
    let extra: usize = filtered.iter().sum::<usize>() - whole.iter().sum::<usize>();
    // The first hit reserves its scratch, and a later, longer source only
    // what it adds.
    assert!(extra >= scratch_bytes(largest), "{extra} {filtered:?}");
    assert!(extra < 2 * scratch_bytes(largest), "{extra} {filtered:?}");
    // Nothing is reserved that is not needed.
    assert!(!filtered.contains(&0), "{filtered:?}");
}

#[test]
fn highlighting_reserves_its_scratch_once_for_the_largest_source() {
    let shard = shard();
    let (_, whole) = respond(&shard, r#"{"sort":[{"n":"asc"}]}"#);
    let (_, lit) = respond(
        &shard,
        r#"{"sort":[{"n":"asc"}],"highlight":{"fields":{"tag":{"no_match_size":3}}}}"#,
    );
    let largest = (0..10)
        .map(|i| doc_source(&spec(), i).len())
        .max()
        .unwrap_or(0);
    let extra: usize = lit.iter().sum::<usize>() - whole.iter().sum::<usize>();
    // The bound of the largest source, tags `<em>` and `</em>` included.
    assert_eq!(extra, scratch_bytes(largest) + largest * 40, "{lit:?}");
    assert!(!lit.contains(&0), "{lit:?}");
}

#[test]
fn the_total_is_written_only_when_tracked() {
    let shard = shard();
    let (tracked, _) = respond(&shard, r#"{"size":0}"#);
    assert!(
        tracked.contains(r#""hits":{"total":{"value":20,"relation":"eq"},"max_score""#),
        "{tracked}"
    );
    let (capped, _) = respond(&shard, r#"{"size":0,"track_total_hits":5}"#);
    assert!(
        capped.contains(r#""total":{"value":5,"relation":"gte"}"#),
        "{capped}"
    );
    let (untracked, _) = respond(&shard, r#"{"size":0,"track_total_hits":false}"#);
    assert!(untracked.contains(r#""hits":{"max_score""#), "{untracked}");
}

#[test]
fn the_hits_of_a_cold_shard_are_fetched_in_one_round() {
    let spec = Spec::new("wide", 4000).segments_of(4000).padded(2000);
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = Arc::new(PooledStore::new(
        Rounds::new(Emulator::new(64 << 20, 1)),
        buffers,
    ));
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(
        PageCache::new(PageCacheConfig {
            page_size: 4096,
            download_window: Some(4096),
            ..PageCacheConfig::default()
        })
        .expect("page cache"),
    );
    let shard = open_shard(store.clone(), cache, "", "wide", 0).expect("opens");
    let source =
        parse_chunks([br#"{"size":30,"query":{"term":{"tag":"t0"}}}"#.as_slice()]).expect("parses");
    let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    store.store().reset(std::time::Duration::from_millis(20));
    let mut out = Vec::new();
    write_response(&mut out, &shard, "idx", "node", &hits, 0, &mut |_| Ok(())).expect("writes");
    let rounds = store.store();
    assert!(rounds.depth() <= 1, "{}", rounds.detail());
}

#[test]
fn the_hits_of_each_segment_are_fetched_in_the_segments_own_round() {
    let spec = Spec::new("wide", 4000).segments_of(2000).padded(2000);
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = Arc::new(PooledStore::new(
        Rounds::new(Emulator::new(64 << 20, 1)),
        buffers,
    ));
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(
        PageCache::new(PageCacheConfig {
            page_size: 4096,
            download_window: Some(4096),
            ..PageCacheConfig::default()
        })
        .expect("page cache"),
    );
    let shard = open_shard(store.clone(), cache, "", "wide", 0).expect("opens");
    let source = parse_chunks([br#"{"size":300,"query":{"term":{"tag":"t0"}}}"#.as_slice()])
        .expect("parses");
    let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    store.store().reset(std::time::Duration::from_millis(20));
    let mut out = Vec::new();
    write_response(&mut out, &shard, "idx", "node", &hits, 0, &mut |_| Ok(())).expect("writes");
    let rounds = store.store();
    // 250 hits over two segments: the chunks of each segment come in the
    // merged reads its announcement makes (three in all, the second segment
    // having a chunk apart), and none is left for the hits themselves.
    assert_eq!(rounds.calls().len(), 3, "{}", rounds.detail());
    assert_eq!(rounds.depth(), 3, "{}", rounds.detail());
}
