//! The memory the extension and training pools take.

use super::{FS, lookup, with};
use crate::boot::config::*;
use oss_mem::Pool;

#[cfg(feature = "extensions")]
#[test]
fn the_extensions_pool_is_taken_from_the_page_cache() {
    // Training takes its own share of the page cache, so this pins the split without it.
    let on = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_EXTENSIONS_PERCENT", "5"),
        ("OSS_KNN_TRAINING_MB", "0"),
    ])))
    .expect("valid");
    let off =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_KNN_TRAINING_MB", "0")]))).expect("valid");
    let (b, o) = (on.budget().expect("fits"), off.budget().expect("fits"));
    assert_eq!(b.pool(Pool::Extensions), on.memory_bytes / 100 * 5);
    assert_eq!(b.pool(Pool::PageCache), on.memory_bytes / 100 * 15);
    assert_eq!(o.pool(Pool::PageCache), off.memory_bytes / 100 * 20);
    let too_much = NodeConfig::from_lookup(lookup(&with(&[("OSS_EXTENSIONS_PERCENT", "21")])));
    assert!(too_much.is_err());
}

#[cfg(not(feature = "extensions"))]
#[test]
fn without_the_feature_the_variable_takes_no_memory() {
    let c =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_EXTENSIONS_PERCENT", "5")]))).expect("valid");
    let b = c.budget().expect("fits");
    let off = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(
        b.pool(Pool::PageCache),
        off.budget().expect("fits").pool(Pool::PageCache)
    );
    assert_eq!(b.pool(Pool::Extensions), 0);
    assert_eq!(b.pool(Pool::PageCache), c.memory_bytes / 100 * 20);
}

// docs/specs/extension-analyzer AC-12
#[test]
fn the_replay_fuel_factor_is_sixteen_and_one_to_a_thousand_and_twenty_four() {
    let read = |v: &str| {
        NodeConfig::from_lookup(lookup(&with(&[("OSS_EXTENSIONS_REPLAY_FUEL_FACTOR", v)])))
    };
    let unset = NodeConfig::from_lookup(lookup(&with(&[]))).expect("valid");
    assert_eq!(unset.extensions_replay_factor, 16);
    assert_eq!(read("1").expect("valid").extensions_replay_factor, 1);
    assert_eq!(read("1024").expect("valid").extensions_replay_factor, 1024);
    assert!(read("0").is_err());
    assert!(read("1025").is_err());
}

// docs/specs/async-search AC-8: the node's own routes reserve results from
// the query pool, which a default node has; the plugin pool it does not.
#[test]
fn a_default_node_has_room_for_the_async_search_registry_in_the_query_pool() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    let b = c.budget().expect("fits");
    let worst = (8 << 20) + (32 << 20) + (72 << 10);
    assert!(b.pool(Pool::QueryExecution) >= worst, "{c:?}");
}

// docs/specs/knn-plugin-api AC-20
const MIB: usize = 1 << 20;

fn config(extra: &[(&str, &str)]) -> NodeConfig {
    NodeConfig::from_lookup(lookup(&with(extra))).expect("valid")
}

#[test]
fn a_big_node_gives_the_training_pool_its_256_mib_out_of_the_page_cache() {
    let c = config(&[("OSS_MEMORY_BYTES", "17179869184")]);
    let b = c.budget().expect("fits");
    let cache = c.memory_bytes / 100 * 20;
    assert_eq!(b.pool(Pool::Training), 256 * MIB);
    assert_eq!(b.pool(Pool::PageCache), cache - 256 * MIB);
}

#[test]
fn a_small_node_gives_it_a_quarter_of_the_page_cache_at_most() {
    let c = config(&[]);
    let b = c.budget().expect("fits");
    let cache = c.memory_bytes / 100 * 20;
    assert_eq!(b.pool(Pool::Training), cache / 4);
    assert_eq!(b.pool(Pool::PageCache), cache - cache / 4);
}

#[test]
fn the_variable_sets_the_size_and_zero_or_knn_off_reserves_none() {
    let c = config(&[
        ("OSS_MEMORY_BYTES", "17179869184"),
        ("OSS_KNN_TRAINING_MB", "64"),
    ]);
    assert_eq!(c.budget().expect("fits").pool(Pool::Training), 64 * MIB);
    let none = config(&[("OSS_KNN_TRAINING_MB", "0")]);
    assert_eq!(none.budget().expect("fits").pool(Pool::Training), 0);
    let off = config(&[("OSS_KNN", "false")]);
    let b = off.budget().expect("fits");
    assert_eq!(b.pool(Pool::Training), 0);
    assert_eq!(b.pool(Pool::PageCache), off.memory_bytes / 100 * 20);
}

#[test]
fn a_size_past_the_bound_is_refused() {
    let past = NodeConfig::from_lookup(lookup(&with(&[("OSS_KNN_TRAINING_MB", "65537")])));
    assert!(past.is_err());
}
