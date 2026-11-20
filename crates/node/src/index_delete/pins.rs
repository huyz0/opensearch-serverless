//! [`ShardPins`] over the live points in time and an optional snapshot source.

use std::sync::Arc;

use oss_meta::{MetaError, PointInTimeStore, ShardPins};

/// Shards of an index incarnation that live snapshots name. The Rust node
/// takes no snapshots, so the default source names none; a source for the
/// snapshots a Java node took is supplied here when one exists.
pub trait SnapshotPins: Send + Sync {
    /// The shards of `uuid` of `index`, below `shards`, a snapshot pins.
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the snapshots cannot be read; nothing is purged.
    fn pinned(&self, index: &str, uuid: &str, shards: u32) -> Result<Vec<u32>, MetaError>;
}

/// No snapshots exist to pin anything.
pub struct NoSnapshots;

impl SnapshotPins for NoSnapshots {
    fn pinned(&self, _: &str, _: &str, _: u32) -> Result<Vec<u32>, MetaError> {
        Ok(Vec::new())
    }
}

/// What keeps a deleted index's shards: points in time, then snapshots.
pub struct NodePins {
    pits: PointInTimeStore,
    snapshots: Arc<dyn SnapshotPins>,
    buffer_bytes: usize,
}

impl NodePins {
    /// Pins read from `pits` through a buffer of `buffer_bytes` (the largest
    /// view record). A view that cannot be read stands in as one pinning
    /// every shard; a listing that fails refuses the purge.
    pub fn new(
        pits: PointInTimeStore,
        snapshots: Arc<dyn SnapshotPins>,
        buffer_bytes: usize,
    ) -> Self {
        Self {
            pits,
            snapshots,
            buffer_bytes,
        }
    }
}

impl ShardPins for NodePins {
    fn pinned(&self, index: &str, uuid: &str, shards: u32) -> Result<Vec<u32>, MetaError> {
        // ALLOC: one buffer a purge, buffer_bytes, reserved by the request.
        let mut buf = vec![0u8; self.buffer_bytes];
        let mut pinned = self.snapshots.pinned(index, uuid, shards)?;
        for view in self.pits.live(&mut buf)? {
            if view.is_placeholder() || (view.capturing && view.pins(index, None)) {
                return Ok((0..shards).collect());
            }
            if view.pins(index, Some(uuid)) {
                pinned.extend(view.shards.iter().map(|s| s.shard));
            }
        }
        Ok(pinned)
    }
}
