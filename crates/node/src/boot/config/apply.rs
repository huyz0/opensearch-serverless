//! What a node's variables set on the assembled node.

use std::time::Duration;

use super::NodeConfig;

/// Gives the assembled node what this node's variables set that it builds
/// from: the write limit and the page cache's disk tier.
pub(in crate::boot) fn apply(from: &NodeConfig, to: &mut crate::assemble::Config) {
    super::write_limit::apply(from, to);
    to.page_cache_disk = from.page_cache.as_ref().map(|d| (d.dir.clone(), d.bytes));
    to.resident_page_bytes = from.resident_page_bytes;
}

impl NodeConfig {
    /// How long a drain waits for the lease to lose the search role.
    pub fn drain_settle(&self) -> Duration {
        Duration::from_millis(self.drain_settle_millis)
    }
}
