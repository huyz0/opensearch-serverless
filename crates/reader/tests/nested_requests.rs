//! docs/specs/reader-inner-hits/SPEC.md AC-6 and
//! docs/specs/reader-nested-sort/SPEC.md AC-7: `inner_hits` on a nested query
//! and a nested sort add no object-store request to a cold search over a
//! nested index, counted by the in-process emulator's meter.

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_reader::dsl::parse_chunks;
use oss_reader::open_shard;
use oss_reader::search::search;
use oss_testkit::index::{Spec, blobs};

/// Requests a cold open plus one search of `body` make on a fresh store.
fn cold_requests(body: &str) -> u64 {
    let spec = Spec::new("nreq", 12).segments_of(4).with_nested();
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = Arc::new(PooledStore::new(Emulator::new(64 << 20, 1), buffers));
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let before = store.store().meter().total();
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    let shard = open_shard(store.clone(), cache, "", &spec.name, 0).expect("opens");
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    assert!(!hits.hits.is_empty(), "{body}");
    store.store().meter().total().saturating_sub(before)
}

#[test]
fn inner_hits_and_a_nested_sort_add_no_request_to_a_cold_search() {
    let plain =
        cold_requests(r#"{"size":20,"query":{"nested":{"path":"c","query":{"match_all":{}}}}}"#);
    let inner = cold_requests(
        r#"{"size":20,"query":{"nested":{"path":"c","query":{"match_all":{}},"inner_hits":{"size":5}}}}"#,
    );
    let sorted = cold_requests(
        r#"{"size":20,"query":{"nested":{"path":"c","query":{"match_all":{}}}},
            "sort":[{"c.w":{"order":"desc","nested":{"path":"c"}}}]}"#,
    );
    assert!(plain > 0, "a cold search makes requests");
    assert_eq!(inner, plain, "inner_hits");
    assert_eq!(sorted, plain, "nested sort");
}
