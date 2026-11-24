//! [`ShardPins`] over the live points in time and an optional snapshot source.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_janitor::JanitorError;
use oss_janitor::orphans::{IndexPins, SnapshotSource};
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

/// The snapshots stored under the deployment's base, read when an index is
/// purged: a shallow snapshot names an index incarnation's blobs without
/// copying them, so a delete must not destroy them while one does.
///
/// Cost of one purge check: one LIST page of `snapshots/` for each 1,000
/// snapshots and one GET for each snapshot, the janitor's own read of the
/// same markers.
pub struct StoreSnapshotPins {
    source: SnapshotSource,
    buffer_bytes: usize,
}

impl StoreSnapshotPins {
    /// The snapshots under `base`, at most `max_snapshots` of them, each
    /// marker read through a buffer of `buffer_bytes`.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        max_snapshots: usize,
        buffer_bytes: usize,
    ) -> Self {
        Self {
            source: SnapshotSource::new(store, base, max_snapshots, buffer_bytes),
            buffer_bytes,
        }
    }
}

impl SnapshotPins for StoreSnapshotPins {
    fn pinned(&self, index: &str, uuid: &str, shards: u32) -> Result<Vec<u32>, MetaError> {
        // ALLOC: one pin set and one buffer a purge, bounded by the snapshot cap, reserved by the request.
        let mut pins = IndexPins::new(usize::MAX);
        let mut buf = vec![0u8; self.buffer_bytes];
        self.source
            .collect_with(&mut pins, &mut buf)
            .map_err(|e| match e {
                JanitorError::Blob(b) => MetaError::Blob(b),
                // Unreadable is not "none": nothing is purged.
                _ => MetaError::InvalidArgument("the snapshots could not be read"),
            })?;
        Ok(if pins.contains(index, uuid) {
            (0..shards).collect()
        } else {
            Vec::new()
        })
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
