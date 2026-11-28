//! The memory the extension pool takes.

use super::{FS, lookup, with};
use crate::boot::config::*;
use oss_mem::Pool;

#[cfg(feature = "extensions")]
#[test]
fn the_extensions_pool_is_taken_from_the_page_cache() {
    let on =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_EXTENSIONS_PERCENT", "5")]))).expect("valid");
    let off = NodeConfig::from_lookup(lookup(FS)).expect("valid");
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
