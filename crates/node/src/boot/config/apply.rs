//! What a node's variables set on the assembled node.

use std::time::Duration;

use super::NodeConfig;

/// Gives the assembled node what this node's variables set that it builds
/// from: the write limit and the page cache's disk tier.
pub(in crate::boot) fn apply(from: &NodeConfig, to: &mut crate::assemble::Config) {
    super::write_limit::apply(from, to);
    // Process-wide, as the engine's read advice is: once, before a shard opens.
    oss_reader::vector_warm::set_at_open(from.vector_warm_at_open);
    oss_cluster::placement::set_ramp_config(oss_cluster::placement::RampConfig {
        min_ms: from.reader_ramp_min_secs.saturating_mul(1000),
        max_ms: from.reader_ramp_max_secs.saturating_mul(1000),
    });
    oss_reader::painless_switch::set_native_compile(from.painless_native_compile);
    to.page_cache_disk = from.page_cache.as_ref().map(|d| (d.dir.clone(), d.bytes));
    to.resident_page_bytes = from.resident_page_bytes;
}

impl NodeConfig {
    /// How long a drain waits for the lease to lose the search role.
    pub fn drain_settle(&self) -> Duration {
        Duration::from_millis(self.drain_settle_millis)
    }
}
