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
