//! The shards a node lists for a joining reader, over a real page cache
//! with a disk tier.

use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use super::SearchService;
use crate::warm_copy::Shards;
use crate::warm_handlers::HeldShards;
use oss_transport::actions::tail::ShardRef;

fn service(dir: &std::path::Path) -> (Arc<SearchService<Emulator>>, Arc<PageCache>) {
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits")
        .with_pool(Pool::OpenIndexes, 32 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for name in ["idx-a", "idx-b"] {
        for (key, bytes) in blobs("", &Spec::new(name, 200)) {
            store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
        }
    }
    let cache = Arc::new(
        PageCache::new(PageCacheConfig {
            disk_dir: Some(dir.to_path_buf()),
            disk_budget: 64 << 20,
            ..PageCacheConfig::default()
        })
        .expect("page cache"),
    );
    let service = SearchService::new(Arc::new(store), Arc::clone(&cache), "", "n1", &budget);
    (Arc::new(service), cache)
}

// docs/specs/reader-warm-copy AC-2
#[test]
fn the_open_shards_are_listed_with_the_prefix_their_cache_keys_start_with() {
    let dir = oss_testkit::TempDir::new("search-warm-held");
    let (service, cache) = service(dir.path());
    assert!(service.held().is_empty(), "nothing is open yet");

    match service.shard("idx-a") {
        Ok(held) => drop(held),
        Err(_) => panic!("idx-a did not open"),
    }
    let held = service.held();

    assert_eq!(held.len(), 1);
    assert_eq!(held[0].shard.index_name, "idx-a");
    assert_eq!(held[0].shard.shard, 0);
    assert!(!held[0].shard.index_uuid.is_empty());
    assert_eq!(held[0].prefix, "/idx-a/0/t=");
    // The prefix is what the pages the open cached are keyed by, so the
    // peer's listing finds them; the other index's prefix finds none.
    let listed = cache.resident_pages(&[held[0].prefix.as_str(), "/idx-b/0/t="], 100);
    assert!(listed[1].is_empty());
    assert!(
        !listed[0].is_empty(),
        "the open cached pages under the prefix"
    );
    for page in &listed[0] {
        assert!(page.key.starts_with(&held[0].prefix));
    }
}

#[test]
fn the_page_cache_a_service_reads_through_is_the_one_it_was_given() {
    let dir = oss_testkit::TempDir::new("search-warm-cache");
    let (service, cache) = service(dir.path());
    assert!(Arc::ptr_eq(service.page_cache(), &cache));
}

fn shard_ref(index: &str, shard: i32) -> ShardRef {
    ShardRef {
        index_name: index.to_owned(),
        index_uuid: String::new(),
        shard,
    }
}

// docs/specs/reader-warm-copy AC-9
#[test]
fn a_shard_the_joiner_opens_accepts_the_pages_its_open_cached_and_no_other_shard_opens() {
    let dir = oss_testkit::TempDir::new("search-warm-expected");
    let (service, cache) = service(dir.path());
    assert!(service.expected(&shard_ref("idx-none", 0)).is_none());
    assert!(service.expected(&shard_ref("idx-a", -1)).is_none());
    assert!(
        service.expected(&shard_ref("idx-a", 5)).is_none(),
        "no such shard"
    );
    assert_eq!(service.held().len(), 0, "a refused name opens nothing");

    let expected = service.expected(&shard_ref("idx-a", 0)).expect("opens");
    assert_eq!(service.held().len(), 1, "asking opened the shard");
    let resident = cache.resident_pages(&["/idx-a/0/t="], 10_000).remove(0);
    let mut accepted = 0;
    for page in &resident {
        let name = page.key.split('\u{0}').nth(1).expect("a name");
        if cache.is_pinned_name(name) || !page.key.contains("\u{0}p\u{0}") {
            continue;
        }
        assert_eq!(
            expected.page_len(&page.key, page.page),
            Some(u64::from(page.len))
        );
        accepted += 1;
    }
    assert!(accepted > 0, "an unpinned page was checked");
}
