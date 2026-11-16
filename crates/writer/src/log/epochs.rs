//! What this node knows of one epoch of its log.

use std::collections::BTreeMap;

use oss_format::{OrdinalRange, ShardKey};

/// One epoch's state (the Java's `NodeLogEpochState`).
#[derive(Debug)]
pub(crate) struct EpochState {
    /// For each shard, the runs of slots holding its unpublished records.
    pub(crate) unpublished: BTreeMap<ShardKey, Vec<OrdinalRange>>,
    /// The term each shard was confirmed at for this epoch.
    pub(crate) admitted: BTreeMap<ShardKey, i64>,
    /// The slot after the last that landed.
    pub(crate) next: u64,
    /// Whether this node still writes it.
    pub(crate) writing: bool,
}

impl Default for EpochState {
    fn default() -> Self {
        Self {
            // ALLOC: one entry a shard with unpublished records in the epoch.
            unpublished: BTreeMap::new(),
            // ALLOC: one entry a shard admitted to the epoch.
            admitted: BTreeMap::new(),
            next: 0,
            writing: true,
        }
    }
}

impl EpochState {
    /// The lowest slot still needed: the first unpublished run's start, or
    /// the next slot when nothing is unpublished.
    pub(crate) fn trim_point(&self) -> u64 {
        self.unpublished
            .values()
            .filter_map(|runs| runs.first().map(|r| r.from))
            .fold(self.next, u64::min)
    }

    /// No longer written and holding nothing unpublished: deletable.
    pub(crate) fn complete(&self) -> bool {
        !self.writing && self.unpublished.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trim_point_is_the_lowest_unpublished_run_or_the_next_slot() {
        let mut state = EpochState {
            next: 9,
            ..EpochState::default()
        };
        assert_eq!(state.trim_point(), 9);
        let run = |from, to| vec![OrdinalRange { from, to }];
        state
            .unpublished
            .insert(ShardKey::new("a", 0, None::<&str>), run(4, 8));
        state
            .unpublished
            .insert(ShardKey::new("b", 0, None::<&str>), run(2, 3));
        assert_eq!(state.trim_point(), 2);
        assert!(!state.complete());
        state.writing = false;
        assert!(!state.complete(), "still holds unpublished records");
        state.unpublished.clear();
        assert!(state.complete());
    }
}
