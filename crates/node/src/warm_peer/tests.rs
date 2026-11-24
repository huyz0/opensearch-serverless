use super::*;
use lucene_store::page_cache::PageCacheConfig;
use oss_transport::actions::warm::PageAsk;

const BASE: &str = "b/idx/0/t=";

fn cache(dir: &std::path::Path) -> PageCache {
    PageCache::new(PageCacheConfig {
        disk_dir: Some(dir.to_path_buf()),
        disk_budget: 64 << 20,
        ..PageCacheConfig::default()
    })
    .expect("cache")
}

fn key(prefix: &str, name: &str, len: u64, tag: char) -> String {
    format!("{prefix}7\0{name}\0{len}\0{tag}")
}

fn held(index: &str) -> HeldShard {
    HeldShard {
        shard: ShardRef {
            index_name: index.into(),
            index_uuid: "u".into(),
            shard: 0,
        },
        prefix: format!("b/{index}/0/t="),
    }
}

fn put(cache: &PageCache, key: &str, page: u64, len: usize) {
    cache.insert_page(key, page, &vec![9; len]);
}

// docs/specs/reader-warm-copy AC-2
#[test]
fn the_hot_set_names_the_pages_of_the_shards_asked_for_and_leaves_pinned_and_decoded_files_out() {
    let dir = oss_testkit::TempDir::new("warm-peer-hot");
    let cache = cache(dir.path());
    let plain = key(BASE.replace("idx", "a").as_str(), "_0.cfs", 100, 'p');
    put(&cache, &plain, 0, 100);
    put(&cache, &key("b/a/0/t=", "segments_3", 100, 'p'), 0, 100);
    put(&cache, &key("b/a/0/t=", "_0.cfd", 100, 'd'), 0, 100);
    put(&cache, &key("b/other/0/t=", "_0.cfs", 100, 'p'), 0, 100);
    let shards = [held("a"), held("other")];
    let only_a = |s: &ShardRef| s.index_name == "a";

    let hot = hot_set(&cache, &shards, only_a, 10, 1 << 20);

    assert_eq!(
        hot.page_size,
        i32::try_from(cache.config().page_size).unwrap()
    );
    assert_eq!(hot.shards.len(), 1);
    let (shard, files) = &hot.shards[0];
    assert_eq!(shard.index_name, "a");
    assert_eq!(
        files,
        &[HotFile {
            key: plain,
            pages: vec![HotPage { page: 0, len: 100 }],
        }]
    );
}

// docs/specs/reader-warm-copy AC-2
#[test]
fn the_hot_set_stops_at_the_shard_count_and_at_the_byte_total() {
    let dir = oss_testkit::TempDir::new("warm-peer-caps");
    let cache = cache(dir.path());
    for index in ["a", "b", "c"] {
        let k = key(&format!("b/{index}/0/t="), "_0.cfs", 300, 'p');
        put(&cache, &k, 0, 100);
        put(&cache, &k, 1, 100);
        put(&cache, &k, 2, 100);
    }
    let shards = [held("a"), held("b"), held("c")];

    let two = hot_set(&cache, &shards, |_| true, 2, 1 << 20);
    assert_eq!(two.shards.len(), 2);

    let pages = |r: &HotSetResponse| -> usize {
        r.shards
            .iter()
            .flat_map(|(_, f)| f)
            .map(|f| f.pages.len())
            .sum()
    };
    // 250 bytes holds two whole 100-byte pages, never a third.
    let small = hot_set(&cache, &shards, |_| true, 10, 250);
    assert_eq!(pages(&small), 2);
    let exact = hot_set(&cache, &shards, |_| true, 10, 300);
    assert_eq!(pages(&exact), 3);
    let none = hot_set(&cache, &shards, |_| true, 10, 0);
    assert_eq!(pages(&none), 0);
}

// docs/specs/reader-warm-copy AC-3
#[test]
fn a_page_ask_is_served_from_the_cache_and_skips_a_page_not_held() {
    let dir = oss_testkit::TempDir::new("warm-peer-serve");
    let cache = cache(dir.path());
    let k = key("b/a/0/t=", "_0.cfs", 100, 'p');
    put(&cache, &k, 0, 4);
    let ask = |page| PageAsk {
        key: k.clone(),
        page,
    };
    let request = PagesRequest {
        asks: vec![ask(5), ask(0)],
        ..PagesRequest::default()
    };

    let served = serve(&cache, &[held("a")], &request).expect("served");

    assert_eq!(
        served.pages,
        vec![PageBytes {
            key: k.clone(),
            page: 0,
            bytes: vec![9; 4],
        }]
    );
}

// docs/specs/reader-warm-copy AC-3
#[test]
fn a_page_ask_naming_a_key_outside_the_held_shards_is_refused_whole() {
    let dir = oss_testkit::TempDir::new("warm-peer-refuse");
    let cache = cache(dir.path());
    let mine = key("b/a/0/t=", "_0.cfs", 100, 'p');
    let theirs = key("b/other/0/t=", "_0.cfs", 100, 'p');
    put(&cache, &mine, 0, 4);
    put(&cache, &theirs, 0, 4);
    let request = PagesRequest {
        asks: vec![
            PageAsk { key: mine, page: 0 },
            PageAsk {
                key: theirs,
                page: 0,
            },
        ],
        ..PagesRequest::default()
    };

    assert_eq!(serve(&cache, &[held("a")], &request), Err(Refused));
}

#[test]
fn a_cache_with_no_disk_tier_has_nothing_hot_and_serves_nothing() {
    let cache = PageCache::new(PageCacheConfig::default()).expect("cache");
    let hot = hot_set(&cache, &[held("a")], |_| true, 10, 1 << 20);
    assert!(hot.shards.iter().all(|(_, files)| files.is_empty()));
    let request = PagesRequest {
        asks: vec![PageAsk {
            key: key("b/a/0/t=", "_0.cfs", 1, 'p'),
            page: 0,
        }],
        ..PagesRequest::default()
    };
    assert!(
        serve(&cache, &[held("a")], &request)
            .expect("served")
            .pages
            .is_empty()
    );
}
