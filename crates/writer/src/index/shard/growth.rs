//! What a shard's writes added to its mapping, kept for the descriptor.

use crate::mapping::Mapping;

use super::Shard;

impl Shard<'_> {
    /// What documents added to the mapping since this was last called, for
    /// the descriptor to hold; none when nothing grew it.
    pub fn take_growth(&mut self) -> Option<Mapping> {
        self.growth.take()
    }

    /// Adds a document's dynamic update to what is pending.
    pub(super) fn note_growth(&mut self, delta: Option<&Mapping>) {
        let Some(delta) = delta else { return };
        // ALLOC: the pending addition, bounded by the total fields limit.
        self.growth = Some(match self.growth.take() {
            Some(pending) => pending.merge(delta).unwrap_or_else(|_| delta.clone()),
            None => delta.clone(),
        });
    }
}
