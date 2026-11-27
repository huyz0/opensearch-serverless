//! A nested index of 10,000 documents (20,000 children) searched: the
//! read side of the soak the nested default flip waits for. Run by hand, as
//! the cost measurements are:
//! `cargo test -p oss-reader --release --lib nested_soak -- --ignored --nocapture`.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use std::sync::Arc;
use std::time::Instant;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use super::filters_render::write_filters;
use crate::aggs::partial::Value;
use crate::dsl::parse_chunks;
use crate::open_shard;
use crate::search::search;

const DOCS: usize = 10_000;

#[test]
#[ignore = "a soak: run by hand"]
fn nested_soak_ten_thousand_documents_answer_exactly() {
    let spec = Spec::new("soak", DOCS).segments_of(2_500).with_nested();
    let config = BudgetConfig::new(256 << 20)
        .with_pool(Pool::Transport, 256 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(512 << 20, 1), buffers);
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    let shard = open_shard(Arc::new(store), cache, "", "soak", 0).expect("opens");
    let body = r#"{"size":0,"query":{"nested":{"path":"c","query":{"match_all":{}}}},
        "aggs":{"n":{"nested":{"path":"c"},"aggs":{"x":{"max":{"field":"c.k"}},
        "v":{"value_count":{"field":"c.k"}},"r":{"reverse_nested":{}}}}}}"#;
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let mut times = Vec::new();
    for _ in 0..5 {
        let at = Instant::now();
        let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
        times.push(at.elapsed());
        assert_eq!(hits.total, DOCS as u64);
        let Value::Filters(r) = &hits.aggregations[0].value else {
            panic!("not a nested");
        };
        let mut out = Vec::new();
        write_filters(&mut out, r).expect("writes");
        let got = String::from_utf8(out).expect("utf-8");
        assert!(got.starts_with(r#""doc_count":20000"#), "{got}");
        assert!(got.contains(r#""v":{"value":20000"#), "{got}");
        assert!(got.contains(r#""x":{"value":99991"#), "{got}");
        assert!(got.contains(r#""r":{"doc_count":10000"#), "{got}");
    }
    times.sort();
    println!(
        "nested soak, 10,000 documents: p50 {:?}, max {:?}",
        times[2], times[4]
    );
}
