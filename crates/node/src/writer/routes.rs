//! The cluster seam a writer routes through.

use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned};
use oss_writer::index::Fetched;

/// What a writer asks of the rest of the cluster: which indexes exist,
/// who owns a shard it does not hold, and how to send that owner a group.
pub trait Routes: Send + Sync {
    /// The index called `name`, if there is one.
    fn index(&self, name: &str) -> Option<IndexInfo>;

    /// Who owns `shard` of `index`, asked when this node does not hold it.
    fn owner(&self, index: &IndexInfo, shard: u32) -> Owner;

    /// Sends a group to the node that owns its shard, once, and answers
    /// each item's outcome in the group's order.
    fn forward(
        &self,
        node: &str,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<ItemResult>;

    /// Asks the node that owns `shard` to refresh it. A route that never
    /// names another owner has nothing to ask.
    fn refresh_remote(&self, _node: &str, _index: &IndexInfo, _shard: u32) -> Result<(), String> {
        Err("the shard is held by another node".to_owned())
    }

    /// Asks the node that owns `shard` for the live document `id`. A route
    /// that never names another owner has nothing to ask.
    fn get_remote(
        &self,
        _node: &str,
        _index: &IndexInfo,
        _shard: u32,
        _id: &str,
    ) -> Result<Option<Fetched>, String> {
        Err("the shard is held by another node".to_owned())
    }

    /// As [`Routes::get_remote`], reading what is not yet refreshed only
    /// when `realtime`. A route that cannot say asks for the realtime read.
    fn get_remote_as(
        &self,
        node: &str,
        index: &IndexInfo,
        shard: u32,
        id: &str,
        _realtime: bool,
    ) -> Result<Option<Fetched>, String> {
        self.get_remote(node, index, shard, id)
    }
}
