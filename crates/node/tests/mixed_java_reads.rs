//! A shard a Rust writer took from the Java shell and published to
//! (`fixtures/mixed-takeover`), which the shell's own classes opened and took
//! back, opened by the Rust reader: both count the same documents and read the
//! same commit and head.

// ARITH: fixture sizes are small constants.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::path::PathBuf;
use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, FsStore, PooledStore};
use oss_format::{CommitManifest, ShardHead};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mixed-takeover")
}

/// The words after `name` on the line of `java.txt` that starts with it.
fn java(name: &str) -> Vec<String> {
    let text = std::fs::read_to_string(fixture().join("java.txt")).expect("java.txt");
    let line = text
        .lines()
        .find(|l| l.starts_with(name))
        .unwrap_or_else(|| panic!("no {name} line"));
    line.split(' ').skip(1).map(str::to_owned).collect()
}

fn number(name: &str) -> u64 {
    java(name)[0].parse().expect("a number")
}

fn store() -> Arc<PooledStore<FsStore>> {
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
    Arc::new(PooledStore::new(
        FsStore::new(fixture().join("store")).expect("store"),
        buffers,
    ))
}

#[test]
fn the_rust_reader_counts_what_the_java_reader_counted_in_the_commit_over_a_java_pack() {
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("cache"));
    let shard = oss_reader::open_shard(store(), cache, "", "fixture", 0).expect("opens");
    assert_eq!(
        u64::try_from(shard.reader.num_docs()).expect("positive"),
        number("num_docs"),
        "live documents"
    );
    assert_eq!(
        u64::try_from(shard.reader.max_doc()).expect("positive"),
        number("max_doc"),
        "documents with the soft-deleted ones"
    );
}

#[test]
fn the_commit_and_head_read_as_the_java_read_them() {
    let store = store();
    let store = store.store();
    let mut buf = vec![0u8; 1 << 16];
    let read = store
        .read_register("segments/fixture#fixture-uuid-0000000#0/manifest", &mut buf)
        .expect("reads")
        .expect("there");
    let manifest = CommitManifest::parse(&buf[..read.len]).expect("parses");
    let printed = java("term");
    assert_eq!(manifest.term.to_string(), printed[0], "term");
    assert_eq!(
        manifest.writer.as_deref(),
        Some(printed[2].as_str()),
        "writer"
    );
    assert_eq!(manifest.files().len().to_string(), printed[4], "files");

    let key = oss_cluster::heads::shard_head_key("", "fixture", 0);
    let read = store
        .read_register(&key, &mut buf)
        .expect("reads")
        .expect("there");
    let head = ShardHead::parse(&buf[..read.len]).expect("parses");
    let before = java("head_before");
    assert_eq!(head.term.to_string(), before[1], "head term");
    assert_eq!(
        head.owner_node_id.as_deref(),
        Some(before[3].as_str()),
        "owner"
    );
    assert_eq!(
        format!("[{}]", head.writers.join(", ")),
        before[5],
        "writers"
    );
}
