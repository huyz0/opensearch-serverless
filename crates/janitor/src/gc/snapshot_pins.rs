//! Snapshots as pins of the per-shard sweep: a shard of an index incarnation a
//! snapshot names keeps every blob.
//!
//! A snapshot record names the indexes it captured, not the blobs, and a
//! capture in progress has no list of them yet, so the whole incarnation is
//! held, which holds at least what the Java's `SnapshotRecord.referencedBlobs`
//! holds. Unreadable is not none: a record that cannot be read fails the read
//! and the pass deletes nothing.

use oss_format::ShardRef;

use super::{Pinned, Pins};
use crate::JanitorError;
use crate::orphans::IndexPins;

/// `inner`'s pins, and every blob of a shard whose index incarnation a
/// snapshot names.
pub struct SnapshotPins<'a> {
    inner: &'a dyn Pins,
    snapshotted: IndexPins,
}

impl<'a> SnapshotPins<'a> {
    /// `inner` with the incarnations in `snapshotted` held whole.
    pub fn new(inner: &'a dyn Pins, snapshotted: IndexPins) -> Self {
        Self { inner, snapshotted }
    }
}

impl Pins for SnapshotPins<'_> {
    /// No request: the snapshot records were read once for the pass.
    fn pinned(&self, shard: &ShardRef<'_>) -> Result<Pinned, JanitorError> {
        if self.snapshotted.contains(shard.index, shard.uuid) {
            return Ok(Pinned::Everything);
        }
        self.inner.pinned(shard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Nothing;

    impl Pins for Nothing {
        fn pinned(&self, _: &ShardRef<'_>) -> Result<Pinned, JanitorError> {
            Ok(Pinned::Blobs(Vec::new()))
        }
    }

    fn shard<'a>(index: &'a str, uuid: &'a str) -> ShardRef<'a> {
        ShardRef {
            index,
            uuid,
            shard: 0,
        }
    }

    #[test]
    fn a_shard_of_a_snapshotted_incarnation_keeps_everything() {
        let mut held = IndexPins::new(10);
        held.add("logs", Some("u1")).expect("fits");
        let pins = SnapshotPins::new(&Nothing, held);
        assert_eq!(
            pins.pinned(&shard("logs", "u1")).expect("answers"),
            Pinned::Everything
        );
    }

    #[test]
    fn another_incarnation_or_index_is_the_inner_pins_answer() {
        let mut held = IndexPins::new(10);
        held.add("logs", Some("u1")).expect("fits");
        let pins = SnapshotPins::new(&Nothing, held);
        for (index, uuid) in [("logs", "u2"), ("metrics", "u1")] {
            assert_eq!(
                pins.pinned(&shard(index, uuid)).expect("answers"),
                Pinned::Blobs(Vec::new()),
                "{index} {uuid}"
            );
        }
    }
}
