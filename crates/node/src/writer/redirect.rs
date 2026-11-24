//! Where a shard another live node holds is written: this node asks that
//! node instead of opening the shard.

use std::collections::HashMap;

use oss_rest::{Exception, Meta};
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned, item_result};

use super::{Failure, OpenRefused, WriterRole, lock, unavailable};

/// The holder of each shard another live node holds, by index uuid and shard.
pub(super) type Redirects = HashMap<(String, u32), String>;

impl WriterRole {
    /// Whether the group's shard is held here; otherwise who owns it.
    pub(crate) fn owner(&self, index: &IndexInfo, shard: u32) -> Owner {
        if self.held(&index.uuid, shard) {
            Owner::Local
        } else if let Some(node) = self
            .redirect(&index.uuid, shard)
            .or_else(|| self.steer(index, shard))
        {
            Owner::Node(node)
        } else {
            self.routes.owner(index, shard)
        }
    }

    /// The live node known to hold a shard this node does not.
    pub(super) fn redirect(&self, uuid: &str, shard: u32) -> Option<String> {
        // ALLOC: one key a lookup.
        lock(&self.redirects)
            .get(&(uuid.to_owned(), shard))
            .cloned()
    }

    /// Sends a group to `node`, forgetting where a shard is held when the
    /// node could not apply it.
    pub(crate) fn forward(
        &self,
        node: &str,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<ItemResult> {
        let results = self.routes.forward(node, index, shard, group);
        if results
            .iter()
            .any(|r| matches!(r, ItemResult::Failed { status, .. } if *status >= 500))
        {
            // ALLOC: one key a failed forward.
            lock(&self.redirects).remove(&(index.uuid.clone(), shard));
        }
        results
    }

    /// Applies a group to a shard held here, staged and acknowledged as a
    /// bulk answers its items.
    pub fn apply_group(
        &self,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<ItemResult> {
        let outcomes = if self.held(&index.uuid, shard) {
            self.group(index, shard, group)
        } else if let Some(holder) = self.redirect(&index.uuid, shard) {
            return self.forward(&holder, index, shard, group);
        } else {
            match self.slot_or_open(index, shard) {
                Ok(slot) => self.group_on(slot, index, shard, group),
                Err(OpenRefused {
                    holder: Some(holder),
                    ..
                }) => {
                    // ALLOC: one entry a shard another node holds.
                    lock(&self.redirects).insert((index.uuid.clone(), shard), holder.clone());
                    return self.forward(&holder, index, shard, group);
                }
                Err(refused) => unavailable(group.len(), || refused.reason.clone()),
            }
        };
        outcomes
            .into_iter()
            .map(|outcome| match outcome {
                Ok(w) => item_result(Ok(w), index, shard),
                Err(Failure::Refused(r)) => item_result(Err(r), index, shard),
                Err(Failure::Throttled(reason)) => ItemResult::Failed {
                    status: 429,
                    error: Exception::new("es_rejected_execution_exception", reason)
                        // ALLOC: three short fields an item that failed.
                        .with_meta("index", Meta::Str(index.name.clone()))
                        .with_meta("shard", Meta::Str(shard.to_string()))
                        .with_meta("index_uuid", Meta::Str(index.uuid.clone())),
                },
                Err(Failure::Unavailable(reason)) => ItemResult::Failed {
                    status: 503,
                    error: Exception::new("node_log_exception", reason)
                        // ALLOC: three short fields an item that failed.
                        .with_meta("index", Meta::Str(index.name.clone()))
                        .with_meta("shard", Meta::Str(shard.to_string()))
                        .with_meta("index_uuid", Meta::Str(index.uuid.clone())),
                },
            })
            .collect()
    }
}
