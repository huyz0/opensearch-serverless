//! docs/specs/node-open-prefetch: the prefetch setting and its memory charge.

use std::collections::BTreeMap;
use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::search::SearchService;
use oss_node::search::open_prefetch::OPEN_PREFETCH_ENABLED;
use oss_reader::{IndexMeta, ShardFiles};
use oss_types::settings::{Live, Snapshot};

use crate::support::{NODE, put_variant, store};

type Store = Arc<PooledStore<Emulator>>;

fn settings_of(value: &str) -> String {
    format!(r#""settings":{{"index.serverless.reader.open_prefetch.enabled":"{value}"}}"#)
}

fn service(store: &Store, pool: usize) -> (SearchService<Emulator>, MemoryBudget, Arc<PageCache>) {
    let budget = MemoryBudget::new(
        &BudgetConfig::new(pool)
            .with_pool(Pool::OpenIndexes, pool)
            .expect("fits"),
    );
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    let service = SearchService::new(Arc::clone(store), Arc::clone(&cache), "", NODE, &budget);
    (service, budget, cache)
}

fn located(store: &Store, index: &str) -> (ShardFiles, IndexMeta) {
    ShardFiles::open_with_meta(store, "", index, 0).expect("locates")
}

fn cluster(values: &[(&str, &str)]) -> Arc<Live> {
    let live = Arc::new(Live::new());
    live.replace(Snapshot {
        generation: 1,
        values: values
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    });
    live
}

#[test]
fn a_descriptors_settings_are_read_into_the_index_meta() {
    let store = store();
    put_variant(&store, "compat-term", "set", |j| {
        j.replace(r#""settings":{}"#, &settings_of("false"))
    });
    assert!(located(&store, "compat-term").1.settings == Default::default());
    let (_, meta) = located(&store, "set");
    assert_eq!(
        oss_types::settings::Source::value(
            &meta.settings,
            "index.serverless.reader.open_prefetch.enabled"
        ),
        Some("false")
    );
}

#[test]
fn the_setting_resolves_index_then_cluster_then_node_then_on() {
    let store = store();
    put_variant(&store, "compat-term", "idx-off", |j| {
        j.replace(r#""settings":{}"#, &settings_of("false"))
    });
    put_variant(&store, "compat-term", "idx-on", |j| {
        j.replace(r#""settings":{}"#, &settings_of("true"))
    });
    let plain = located(&store, "compat-term").1;
    let off = located(&store, "idx-off").1;
    let on = located(&store, "idx-on").1;
    let (service, _, _) = service(&store, 64 << 20);
    assert!(service.prefetch_enabled(&plain), "on before any layer");
    let node_off = BTreeMap::from([(OPEN_PREFETCH_ENABLED.key().to_owned(), "false".to_owned())]);
    assert!(service.set_settings(cluster(&[]), node_off));
    assert!(!service.prefetch_enabled(&plain), "the node's value");
    assert!(service.prefetch_enabled(&on), "an index's value wins");
    assert!(!service.prefetch_enabled(&off));
    let (service, _, _) = self::service(&store, 64 << 20);
    let live = cluster(&[(OPEN_PREFETCH_ENABLED.key(), "false")]);
    service.set_settings(Arc::clone(&live), BTreeMap::new());
    assert!(!service.prefetch_enabled(&plain), "the cluster's value");
    assert!(service.prefetch_enabled(&on));
    live.replace(Snapshot {
        generation: 2,
        values: BTreeMap::new(),
    });
    assert!(service.prefetch_enabled(&plain), "a newer register is read");
}

#[test]
fn a_prefetching_open_holds_its_bytes_until_it_returns() {
    let store = store();
    let (service, budget, _) = service(&store, 64 << 20);
    let (files, meta) = located(&store, "compat-term");
    let (config, held) = service.open_prefetch(&files, &meta);
    assert!(config.enabled);
    let held_bytes = budget.in_use(Pool::OpenIndexes);
    assert!(held_bytes > 0, "the plan's bytes are charged");
    drop(held);
    assert_eq!(budget.in_use(Pool::OpenIndexes), 0, "given back");
    service.open("compat-term", 0).expect("opens");
    let steady = budget.in_use(Pool::OpenIndexes);
    assert!(steady > 0);
    assert!(
        steady < held_bytes + steady,
        "the open shard holds its steady charge, not the prefetch's"
    );
}

#[test]
fn a_pool_with_no_room_opens_without_prefetch() {
    let store = store();
    let (files, meta) = located(&store, "compat-term");
    let (probe, probe_budget, cache) = service(&store, 64 << 20);
    let (_, held) = probe.open_prefetch(&files, &meta);
    let prefetch_bytes = probe_budget.in_use(Pool::OpenIndexes);
    drop(held);
    let steady = oss_reader::open_cost::open_bytes(&files, &meta, &cache);
    assert!(prefetch_bytes > 0 && steady > 0);
    let (tight, budget, _) = service(&store, steady);
    // The open takes its steady charge first, which fills the pool.
    tight.open("compat-term", 0).expect("still opens");
    assert_eq!(budget.in_use(Pool::OpenIndexes), steady);
    let (config, held) = tight.open_prefetch(&files, &meta);
    assert!(!config.enabled, "no room, so no prefetch");
    assert!(held.is_none());
    assert_eq!(budget.in_use(Pool::OpenIndexes), steady);
}

#[test]
fn an_index_that_turns_it_off_holds_nothing() {
    let store = store();
    put_variant(&store, "compat-term", "off", |j| {
        j.replace(r#""settings":{}"#, &settings_of("false"))
    });
    let (service, budget, _) = service(&store, 64 << 20);
    let (files, meta) = located(&store, "off");
    let (config, held) = service.open_prefetch(&files, &meta);
    assert!(!config.enabled && held.is_none());
    assert_eq!(budget.in_use(Pool::OpenIndexes), 0);
}
