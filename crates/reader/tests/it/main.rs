//! The reader opens commits the Java shell published (fixtures/shell-blobs),
//! through the in-process emulator.

// ARITH: test sizes and counts are small constants; an overflow panics the
// test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

mod frozen;
mod frozen_bucket;
mod open;
mod open_bucket;
mod push;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, Emulator, PooledStore, S3Op, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};

/// The index the fixtures hold.
pub const INDEX: &str = "fixture";
/// The deployment base the fixtures were written under.
pub const BASE: &str = "";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/shell-blobs")
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).expect("fixture dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if path.file_name().is_some_and(|n| n != "README.md") {
            let key = path
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((key, std::fs::read(&path).expect("read fixture")));
        }
    }
}

/// An emulator holding every fixture blob at its key, behind a buffer pool.
/// Loading them counts as PUTs; tests compare counts before and after.
pub fn store_with_fixtures() -> Arc<PooledStore<Emulator>> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    let mut blobs = Vec::new();
    let root = fixtures();
    walk(&root, &root, &mut blobs);
    assert!(blobs.len() >= 4, "fixtures missing: {blobs:?}");
    for (key, bytes) in &blobs {
        store
            .put(key, &mut SliceSource::new(bytes))
            .expect("load fixture");
    }
    Arc::new(store)
}

/// A memory-only page cache.
pub fn cache() -> Arc<PageCache> {
    Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"))
}

/// Every request kind's count so far.
pub fn counts(store: &PooledStore<Emulator>) -> [u64; 7] {
    S3Op::ALL.map(|op| store.store().meter().count(op))
}

/// The requests made since `before`, per kind.
pub fn since(store: &PooledStore<Emulator>, before: [u64; 7]) -> Vec<(S3Op, u64)> {
    let now = counts(store);
    S3Op::ALL
        .iter()
        .zip(now.iter().zip(before.iter()))
        .filter(|(_, (n, b))| n > b)
        .map(|(op, (n, b))| (*op, n - b))
        .collect()
}
