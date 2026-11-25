//! Merging a shard's small segments after a refresh, under a guard the node
//! grants and the shard holds for as long as the merge runs.

use std::sync::Arc;

use lucene_index::merge_policy::MergePolicyConfig;

use super::{Shard, message};

/// Held for as long as a merge runs; dropping it gives back the node's
/// permission and the memory reserved for the merge.
pub trait MergeGuard: Send {}

impl<T: Send> MergeGuard for T {}

/// What lets a shard merge: the node's permit and a memory reservation.
pub trait MergeGate: Send + Sync {
    /// Starts a merge that can hold `bytes` at its peak, or `None` when the
    /// node will not allow one now. It never waits: a shard refused tries at
    /// its next refresh.
    fn try_start(&self, bytes: u64) -> Option<Box<dyn MergeGuard>>;
}

/// How a shard's segments merge.
#[derive(Clone, Debug)]
pub struct MergeSettings {
    /// Segments one tier holds before a merge is due.
    pub segments_per_tier: usize,
    /// Segments one merge takes at most.
    pub max_merge_at_once: usize,
    /// The size past which a segment is not merged, and so the most one
    /// merge writes.
    pub max_merged_segment_bytes: u64,
}

/// A shard's merge settings and the gate it asks.
pub(super) struct Merging {
    settings: MergeSettings,
    gate: Arc<dyn MergeGate>,
}

impl<'d> Shard<'d> {
    /// Merges this shard's small segments after each refresh, when `gate`
    /// allows it.
    #[must_use]
    pub fn with_merging(mut self, settings: MergeSettings, gate: Arc<dyn MergeGate>) -> Self {
        self.merging = Some(Merging { settings, gate });
        self
    }

    /// Merges small segments when a tier is full and the gate allows it, and
    /// reports whether a merge ran. The writer is opened with no merge
    /// policy, so `commit` never merges; the policy is installed only while
    /// the guard is held, and a merge changes the writer's view alone: the
    /// caller commits again to make it durable.
    pub(super) fn merge_small_segments(&mut self) -> Result<bool, String> {
        let Some(merging) = &self.merging else {
            return Ok(false);
        };
        let before = self.writer.segment_infos().segments.len();
        if before <= merging.settings.segments_per_tier {
            return Ok(false);
        }
        // Segments at half the cap or more are left alone by the policy, so
        // only the smaller ones can merge and only they count.
        let (mergeable, held) = self.mergeable()?;
        if mergeable <= merging.settings.segments_per_tier {
            return Ok(false);
        }
        // A merge holds its inputs and its output at once; merges in one call
        // run one after another, each at most the cap.
        let peak = held
            .min(merging.settings.max_merged_segment_bytes)
            .saturating_mul(2);
        let Some(guard) = merging.gate.try_start(peak) else {
            return Ok(false);
        };
        #[allow(clippy::cast_precision_loss)]
        let config = MergePolicyConfig {
            segments_per_tier: merging.settings.segments_per_tier as f64,
            max_merge_at_once: merging.settings.max_merge_at_once,
            max_merged_segment_size: merging.settings.max_merged_segment_bytes,
            ..MergePolicyConfig::default()
        };
        self.writer.set_merge_policy(Some(config));
        let merged = self.writer.maybe_merge();
        self.writer.set_merge_policy(None);
        drop(guard);
        merged.map_err(|e| message(&e))?;
        Ok(self.writer.segment_infos().segments.len() < before)
    }

    /// How many segments are under half the merge cap, and their bytes.
    fn mergeable(&self) -> Result<(usize, u64), String> {
        let limit = self
            .merging
            .as_ref()
            .map_or(0, |m| m.settings.max_merged_segment_bytes / 2);
        let (mut count, mut total) = (0usize, 0u64);
        for info in &self.writer.segment_infos().segments {
            let mut bytes = 0u64;
            for name in lucene_index::index_file_deleter::segment_files(self.dir, info)
                .map_err(|e| message(&e))?
            {
                bytes = bytes.saturating_add(self.dir.file_length(&name).map_err(|e| message(&e))?);
            }
            if bytes < limit {
                count = count.saturating_add(1);
                total = total.saturating_add(bytes);
            }
        }
        Ok((count, total))
    }
}
