//! Where a shard another live node holds is written: this node asks that
//! node instead of opening the shard.

use std::collections::HashMap;

use oss_rest::{Exception, Meta};
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned, item_result};
use oss_writer::index::Written;

use super::{Failure, NOT_OWNER, OpenRefused, WriterRole, lock, unavailable};

/// The holder of each shard another live node holds, by index uuid and shard.
pub(super) type Redirects = HashMap<(String, u32), String>;

impl WriterRole {
    /// Whether the group's shard is held here; otherwise who owns it.
    pub(crate) fn owner(&self, index: &IndexInfo, shard: u32) -> Owner {
        if self.held(&index.uuid, shard) {
            Owner::Local
        } else if let Some(holder) = self.redirect(&index.uuid, shard) {
            Owner::Node(holder)
        } else {
            // Steering only moves a shard the routes would open here: one
            // another node owns stays where the routes put it.
            match self.routes.owner(index, shard) {
                Owner::Local => self.steer(index, shard).map_or(Owner::Local, Owner::Node),
                owner @ Owner::Node(_) => owner,
            }
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
            self.unsteer(&index.uuid, shard);
        }
        if results.first().is_some_and(refused_as_not_owner) {
            return self.take_after_refusal(node, index, shard, group, results);
        }
        results
    }

    /// A peer that answered it does not own the shard: the placement named it,
    /// but the shard head does not. Reads the head as a write would and
    /// applies the group where it names, once; if the head names no one else,
    /// this node takes the shard, as the peer's own convention for a refused
    /// steered write says the sender does.
    fn take_after_refusal(
        &self,
        node: &str,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
        refused: Vec<ItemResult>,
    ) -> Vec<ItemResult> {
        match self.slot_or_open(index, shard) {
            Ok(slot) => {
                let outcomes = self.group_on(slot, index, shard, group);
                items_of(outcomes, index, shard)
            }
            Err(OpenRefused {
                holder: Some(holder),
                ..
            }) if holder != node => {
                // ALLOC: one entry a shard another node holds.
                lock(&self.redirects).insert((index.uuid.clone(), shard), holder.clone());
                self.routes.forward(&holder, index, shard, group)
            }
            Err(_) => refused,
        }
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
        items_of(outcomes, index, shard)
    }
}

/// Whether a forward was refused because the peer does not own the shard.
fn refused_as_not_owner(result: &ItemResult) -> bool {
    matches!(result, ItemResult::Failed { error, .. } if error.kind() == NOT_OWNER)
}

/// The type a refused write answers with: a pinned module that could not run
/// the shard's open or its replay is named, so a caller can tell it from the
/// node log's own failures.
fn unavailable_kind(reason: &str) -> &'static str {
    if reason.starts_with("extension_replay_failed") {
        "extension_replay_failed"
    } else if reason.starts_with("extension_unavailable") {
        "extension_unavailable"
    } else {
        "node_log_exception"
    }
}

/// The items of a group as a bulk answers them.
fn items_of(
    outcomes: Vec<Result<Written, Failure>>,
    index: &IndexInfo,
    shard: u32,
) -> Vec<ItemResult> {
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
                error: Exception::new(unavailable_kind(&reason), reason)
                    // ALLOC: three short fields an item that failed.
                    .with_meta("index", Meta::Str(index.name.clone()))
                    .with_meta("shard", Meta::Str(shard.to_string()))
                    .with_meta("index_uuid", Meta::Str(index.uuid.clone())),
            },
        })
        .collect()
}
