//! What the log still holds of each shard, for publishes, the lease and
//! cleanup.

use std::collections::BTreeMap;

use oss_format::{NodeLogEpoch, OrdinalRange, ShardKey, trim_below};

use super::{Inner, NodeLogCommitter, Position, epoch_entry};

impl NodeLogCommitter {
    /// Records that a shard published everything up to `up_to`, a position
    /// [`NodeLogCommitter::commit`] returned for it or
    /// [`NodeLogCommitter::last_position`], so the log no longer needs it.
    pub fn on_published(&self, shard: &ShardKey, up_to: Position) {
        let mut inner = self.lock();
        for (epoch, state) in inner.epochs.range_mut(..=up_to.epoch) {
            let Some(runs) = state.unpublished.get(shard) else {
                continue;
            };
            let kept = if *epoch < up_to.epoch {
                // ALLOC: none: an empty vector does not allocate.
                Vec::new()
            } else {
                trim_below(runs, up_to.ordinal.saturating_add(1))
            };
            if kept.is_empty() {
                state.unpublished.remove(shard);
            } else {
                state.unpublished.insert(shard.clone(), kept);
            }
        }
    }

    /// Where a shard's last records landed, across every epoch this node
    /// tracks; `None` when the log holds nothing unpublished of it.
    pub fn last_position(&self, shard: &ShardKey) -> Option<Position> {
        let inner = self.lock();
        inner.epochs.iter().rev().find_map(|(epoch, state)| {
            state.unpublished.get(shard)?.last().map(|run| Position {
                epoch: *epoch,
                ordinal: run.to,
            })
        })
    }

    /// For every epoch this node tracks, the runs holding a shard's
    /// unpublished records, oldest first: what this node reads to reopen
    /// one of its own shards from the log.
    pub fn unpublished_runs(&self, shard: &ShardKey) -> BTreeMap<u64, Vec<OrdinalRange>> {
        let inner = self.lock();
        inner
            .epochs
            .iter()
            .filter_map(|(epoch, state)| {
                let runs = state.unpublished.get(shard).filter(|r| !r.is_empty())?;
                Some((*epoch, runs.clone()))
            })
            // ALLOC: one entry an epoch holding the shard, a handful.
            .collect()
    }

    /// Forgets that a shard was admitted, so its next records in any epoch
    /// are checked again: for a shard this node let go of.
    pub fn forget(&self, shard: &ShardKey) {
        let mut inner = self.lock();
        for state in inner.epochs.values_mut() {
            state.admitted.remove(shard);
        }
    }

    /// Every epoch a successor would have to fence and replay, for the
    /// lease: the one being written, and every earlier one still holding
    /// unpublished records, oldest first.
    pub fn positions(&self) -> Vec<NodeLogEpoch> {
        Self::positions_locked(&self.lock())
    }

    pub(super) fn positions_locked(inner: &Inner) -> Vec<NodeLogEpoch> {
        // ALLOC: one entry an open epoch, a handful.
        let mut out: Vec<NodeLogEpoch> = inner
            .epochs
            .iter()
            .filter(|(_, state)| !state.complete())
            .map(|(epoch, state)| epoch_entry(*epoch, state.trim_point(), state.next))
            // ALLOC: one entry an open epoch, a handful.
            .collect();
        if let Some(opening) = inner.opening {
            out.push(epoch_entry(opening, 0, 0));
        }
        out
    }

    /// Removes and returns the epochs no longer written and holding nothing
    /// unpublished, for deletion: every slot up to and including each one's
    /// `landed`, since that slot is the one an append of unknown outcome
    /// may have taken.
    pub fn take_complete(&self) -> Vec<NodeLogEpoch> {
        let mut inner = self.lock();
        let complete: Vec<u64> = inner
            .epochs
            .iter()
            .filter(|(_, state)| state.complete())
            .map(|(epoch, _)| *epoch)
            // ALLOC: the complete epochs, a handful.
            .collect();
        complete
            .into_iter()
            .filter_map(|epoch| {
                let state = inner.epochs.remove(&epoch)?;
                Some(epoch_entry(epoch, state.next, state.next))
            })
            // ALLOC: as above.
            .collect()
    }

    /// The trim point of the epoch being written, the lowest slot it may
    /// still need; `None` when no epoch is being written.
    pub fn trim_point(&self) -> Option<u64> {
        let inner = self.lock();
        inner
            .current
            .and_then(|c| inner.epochs.get(&c))
            .map(crate::log::epochs::EpochState::trim_point)
    }
}
