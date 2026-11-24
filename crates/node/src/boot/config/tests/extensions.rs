//! The memory the extension pool takes.

use super::{FS, lookup, with};
use crate::boot::config::*;

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
