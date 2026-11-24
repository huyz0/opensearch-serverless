//! The object-store requests a `knn` search makes over vectors of a
//! realistic size, against the same index's term search, through the
//! in-process emulator (which meters every call as the S3 client makes it).
//! Not pass/fail: it prints what `docs/knn-measurements.md` records, and is
//! ignored so `cargo test` does not run it.
//!
//! ```text
//! KNN_DOCS=100000 KNN_DIM=128 KNN_SEGMENT=10000 \
//!   cargo test -p oss-reader --release --test knn_requests -- --ignored --nocapture
//! ```

// ARITH: measurement code over small counts.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_methods,
    clippy::print_stdout
)]

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_codecs::field_infos::VectorSimilarityFunction;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, Emulator, PooledStore, S3Op, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_reader::dsl::parse_chunks;
use oss_reader::open_shard;
use oss_reader::search::search;
use oss_testkit::index::{Spec, blobs, spec_vector};

static BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn counts(store: &PooledStore<Emulator>) -> [u64; 7] {
    S3Op::ALL.map(|op| store.store().meter().count(op))
}

fn since(store: &PooledStore<Emulator>, before: [u64; 7]) -> String {
    let now = counts(store);
    let bytes = store.store().meter().bytes_read();
    let delta = bytes - BYTES.swap(bytes, std::sync::atomic::Ordering::Relaxed);
    let head = format!("bytes_read={delta} ");
    head + &S3Op::ALL
        .iter()
        .zip(now.iter().zip(before.iter()))
        .filter(|(_, (n, b))| n > b)
        .map(|(op, (n, b))| format!("{op:?}={}", n - b))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
#[ignore = "a measurement, run by hand"]
fn requests() {
    let docs = env("KNN_DOCS", 100_000);
    let dim = env("KNN_DIM", 128);
    let segment = env("KNN_SEGMENT", 10_000);
    let spec = Spec::new("kn", docs)
        .segments_of(segment)
        .with_vectors(VectorSimilarityFunction::Euclidean)
        .with_vector_dimension(dim);
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::Transport, 64 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = Arc::new(PooledStore::new(Emulator::new(1 << 30, 1), buffers));
    let mut bytes = 0usize;
    for (key, blob) in blobs("", &spec) {
        bytes += blob.len();
        store.put(&key, &mut SliceSource::new(&blob)).expect("put");
    }
    let vector: Vec<String> = spec_vector(&spec, 1)
        .expect("vector")
        .iter()
        .map(f32::to_string)
        .collect();
    let knn = format!(
        r#"{{"query":{{"knn":{{"v":{{"vector":[{}],"k":10}}}}}},"size":10}}"#,
        vector.join(",")
    );
    let term = r#"{"query":{"term":{"tag":"t3"}},"size":10}"#.to_owned();
    println!(
        "config: docs={docs} dim={dim} segment={segment} index bytes={}",
        bytes
    );
    println!(
        "demand paging available on this host: {}",
        lucene_store::page_cache::demand_paging_available()
    );
    let analyzer = Analyzer::standard(None);
    for (label, body) in [("term", &term), ("knn", &knn)] {
        // Each row opens the shard with an empty page cache: a cold node.
        let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
        let before = counts(&store);
        let shard = open_shard(store.clone(), cache, "", "kn", 0).expect("opens");
        println!("{label}: open: {}", since(&store, before));
        let source = parse_chunks([body.as_bytes()]).expect("parses");
        let before = counts(&store);
        let hits = search(&shard, "kn", &source, &analyzer).expect("search");
        println!(
            "{label}: first search ({} hits): {}",
            hits.hits.len(),
            since(&store, before)
        );
        let before = counts(&store);
        search(&shard, "kn", &source, &analyzer).expect("search");
        println!("{label}: second search: {}", since(&store, before));
    }
}
