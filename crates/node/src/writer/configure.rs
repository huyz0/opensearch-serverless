//! What the node's configuration sets on the writer role.

use oss_mem::MemoryBudget;

use super::{ShardCap, WriterRole};
use crate::assemble::Config;

impl WriterRole {
    /// Applies the write limit, hand-off, shard cap and merging settings of
    /// `config`.
    pub(crate) fn configure(&mut self, config: &Config, budget: &MemoryBudget) {
        self.set_write_limiter(config.write_limit.build());
        self.set_hand_off(config.hand_off);
        self.set_evict_headroom_percent(config.evict_headroom_percent);
        self.set_pinned_indexes(config.pinned_indexes.iter().cloned());
        self.set_shard_cap(ShardCap {
            max: config.max_shards,
            evict_after_millis: config.evict_after_millis,
        });
        self.set_merging(&config.merge, budget);
    }
}
