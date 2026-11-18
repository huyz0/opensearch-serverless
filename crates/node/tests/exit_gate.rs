//! A commit the Rust writer published, which the shell's own classes and a
//! Lucene reader opened (`fixtures/exit-gate`), opened by the Rust reader:
//! both readers count the same files and documents in the same bytes.

// ARITH: fixture sizes are small; an overflow panics the test, which is the
// failure wanted. The fixture files are a few kilobytes, read whole.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::path::PathBuf;
use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{FsStore, PooledStore};
use oss_format::CommitManifest;
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/exit-gate/rust-commit")
}

/// What `JavaOpensRust` printed for the commit: `term`, `num_docs`, `max_doc`
/// and the file count, by name.
fn java(name: &str) -> u64 {
    let text = std::fs::read_to_string(fixture().join("java.txt")).expect("java.txt");
    let line = text
        .lines()
        .find(|l| l.starts_with(name))
        .unwrap_or_else(|| panic!("no {name} line"));
    let words: Vec<&str> = line.split(' ').collect();
    let at = if name == "term" {
        words.iter().position(|w| *w == "files").expect("files") + 1
    } else {
        1
    };
    words[at].parse().expect("a number")
}

#[test]
fn the_rust_reader_counts_what_the_java_reader_counted_in_a_rust_written_commit() {
    let config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers = BufferPool::new(
        &MemoryBudget::new(&config),
        Pool::Transport,
        SMALL_BUFFER,
        4,
    )
    .expect("fits");
    let store = Arc::new(PooledStore::new(
        FsStore::new(fixture().join("store")).expect("store"),
        buffers,
    ));
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("cache"));
    let shard = oss_reader::open_shard(store, cache, "", "logs", 0).expect("opens");
    assert_eq!(
        u64::try_from(shard.reader.num_docs()).expect("positive"),
        java("num_docs"),
        "live documents"
    );
    assert_eq!(
        u64::try_from(shard.reader.max_doc()).expect("positive"),
        java("max_doc"),
        "documents with the soft-deleted ones"
    );
    let manifest = std::fs::read(fixture().join("manifest.bin")).expect("manifest.bin");
    let manifest = CommitManifest::parse(&manifest).expect("parses");
    assert_eq!(manifest.term, 3);
    assert_eq!(manifest.writer.as_deref(), Some("node-1"));
    assert_eq!(
        u64::try_from(manifest.files().len()).expect("small"),
        java("term"),
        "files"
    );
}
