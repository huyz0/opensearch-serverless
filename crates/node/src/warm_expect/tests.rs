use super::*;
use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_reader::ShardFiles;
use oss_testkit::index::{Spec, blobs};

struct Rig {
    store: Arc<PooledStore<Emulator>>,
    cache: Arc<PageCache>,
    files: ShardFiles,
    namespace: String,
}

fn rig(dir: &std::path::Path, page_size: usize) -> Rig {
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", &Spec::new("idx", 400)) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let store = Arc::new(store);
    let cache = Arc::new(
        PageCache::new(PageCacheConfig {
            disk_dir: Some(dir.to_path_buf()),
            disk_budget: 64 << 20,
            page_size,
            ..PageCacheConfig::default()
        })
        .expect("cache"),
    );
    let files = ShardFiles::open(&store, "", "idx", 0).expect("locate");
    let namespace = format!("/idx/0/t={}", files.term);
    Rig {
        store,
        cache,
        files,
        namespace,
    }
}

fn expected(rig: &Rig) -> Expected {
    Expected::new(
        &rig.namespace,
        &rig.files,
        rig.cache.config().page_size,
        &|n| rig.cache.is_pinned_name(n),
    )
}

/// The shard opened and searched through the cache, so it holds the pages a
/// search reads.
fn warm(rig: &Rig) {
    let (files, meta) = ShardFiles::open_with_meta(&rig.store, "", "idx", 0).expect("locate");
    let shard = oss_reader::open_located(
        Arc::clone(&rig.store),
        Arc::clone(&rig.cache),
        "",
        "idx",
        0,
        files,
        meta,
    )
    .expect("opens");
    drop(shard);
}

// docs/specs/reader-warm-copy AC-9
#[test]
fn every_page_an_open_caches_is_one_the_joiner_would_accept_at_its_own_length() {
    let dir = oss_testkit::TempDir::new("warm-expect-real");
    // A small page so a file spans several.
    let rig = rig(dir.path(), 1024);
    warm(&rig);
    let accepted = expected(&rig);
    let prefix = format!("{}\u{0}", rig.namespace);
    let resident = rig
        .cache
        .resident_pages(&[prefix.as_str()], 10_000)
        .remove(0);
    assert!(!resident.is_empty(), "the open cached pages");
    let mut checked = 0;
    for page in &resident {
        let name = page.key.split('\u{0}').nth(1).expect("a name");
        if rig.cache.is_pinned_name(name) {
            continue;
        }
        assert_eq!(
            accepted.page_len(&page.key, page.page),
            Some(u64::from(page.len)),
            "{name} page {}",
            page.page
        );
        checked += 1;
    }
    assert!(checked > 0, "an unpinned page was checked");
}

// docs/specs/reader-warm-copy AC-9
#[test]
fn a_page_outside_what_the_open_commit_stores_is_refused() {
    let dir = oss_testkit::TempDir::new("warm-expect-refuse");
    let rig = rig(dir.path(), 1024);
    let accepted = expected(&rig);
    let name = rig
        .files
        .names()
        .find(|n| !rig.cache.is_pinned_name(n) && rig.files.get(n).is_some_and(|f| f.length > 1024))
        .expect("an unpinned file of two pages")
        .to_owned();
    let range = rig.files.get(&name).expect("range").clone();
    let key = format!(
        "{}\u{0}{name}\u{0}{}\u{0}p\u{0}v{}:{}",
        rig.namespace,
        range.length,
        range.key.len(),
        range.key
    );
    let pages = range.length.div_ceil(1024);
    let last = pages - 1;
    let last_len = range.length - last * 1024;

    assert_eq!(accepted.page_len(&key, 0), Some(1024));
    assert_eq!(accepted.page_len(&key, last), Some(last_len));
    assert!(accepted.accepts(&key, 0, 1024));
    // One past the last page, and the largest page number.
    assert_eq!(accepted.page_len(&key, pages), None);
    assert_eq!(accepted.page_len(&key, u64::from(u32::MAX)), None);
    assert_eq!(accepted.page_len(&key, u64::MAX), None);
    // The right page with the wrong number of bytes, either way.
    assert!(!accepted.accepts(&key, 0, 1023));
    assert!(!accepted.accepts(&key, 0, 1025));
    assert!(!accepted.accepts(&key, last, usize::try_from(last_len).unwrap() + 1));
    // Another term, another file length, another blob version, a decoded
    // file's key and a key of another shard.
    for other in [
        key.replace("/t=", "/t=9"),
        key.replace(&format!("\u{0}{}\u{0}p", range.length), "\u{0}1\u{0}p"),
        key.replace("\u{0}v", "\u{0}vX"),
        key.replacen("\u{0}p\u{0}", "\u{0}d\u{0}", 1),
        key.replacen("/idx/", "/other/", 1),
    ] {
        assert_eq!(accepted.page_len(&other, 0), None, "{other:?}");
    }
}

// docs/specs/reader-warm-copy AC-9
#[test]
fn pinned_names_are_never_accepted_and_segments_files_are_pinned() {
    let dir = oss_testkit::TempDir::new("warm-expect-pinned");
    let rig = rig(dir.path(), 1024);
    let accepted = expected(&rig);
    let pinned = rig
        .files
        .names()
        .find(|n| rig.cache.is_pinned_name(n))
        .expect("a pinned file")
        .to_owned();
    let range = rig.files.get(&pinned).expect("range");
    let key = format!(
        "{}\u{0}{pinned}\u{0}{}\u{0}p\u{0}v{}:{}",
        rig.namespace,
        range.length,
        range.key.len(),
        range.key
    );
    assert_eq!(accepted.page_len(&key, 0), None);
    assert!(accepted.files() > 0);
    assert!(accepted.files() < rig.files.len());
}
