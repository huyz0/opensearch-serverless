//! The pins points in time keep: the blobs a frozen view names.
//!
//! Ported from the shell's `GarbageCollector` use of `livePointsInTime`
//! (`PointInTime.pins`, `referencedBlobs`, `isCapturing`, `isPlaceholder`).
//! A pass reads the live views once, then answers each shard of the pass
//! from memory: no request per shard. That is safe under the two-sweep
//! grace: a blob the sweep may delete has been unreferenced by the live
//! commit since an earlier sweep, so no view taken after that earlier
//! sweep's snapshot can name it, and a view taken before it is in this
//! snapshot. It holds only if a sweep that deletes uses a snapshot taken
//! after the sweep that first saw the blob, so a snapshot answers each
//! shard once: asked again, it says [`Pinned::Everything`] and nothing is
//! deleted on its word.

use std::collections::HashSet;
use std::sync::Mutex;

use oss_format::{FileLocation, PointInTime, ShardRef};
use oss_meta::PointInTimeStore;

use super::{Pinned, Pins};
use crate::JanitorError;

/// The live points in time as of one read, answering [`Pins`] for every
/// shard of a pass without touching the store.
pub struct PointInTimePins {
    base: String,
    views: Vec<PointInTime>,
    // The shards this snapshot has answered, by storage path.
    answered: Mutex<HashSet<String>>,
}

impl PointInTimePins {
    /// Reads the live views of the deployment under `base` through `buf`,
    /// which must hold the largest record: a view that does not fit reads
    /// as one that pins everything. Claims the expiry of the views that have
    /// expired, as [`PointInTimeStore::live`] does.
    ///
    /// Cost: one LIST page per 1,000 views, and a register read for each.
    ///
    /// # Errors
    ///
    /// [`JanitorError::Meta`] when the listing fails; [`JanitorError::Pins`]
    /// when more than `max_views` are live, so the pass deletes nothing
    /// rather than hold them all. (The listing has built them by then: the
    /// cap refuses to keep them, it does not bound the read.)
    pub fn snapshot(
        store: &PointInTimeStore,
        base: &str,
        max_views: usize,
        buf: &mut [u8],
    ) -> Result<Self, JanitorError> {
        let views = store.live(buf).map_err(JanitorError::Meta)?;
        if views.len() > max_views {
            return Err(JanitorError::Pins(
                "more points in time are live than the cap",
            ));
        }
        Ok(Self::of(base, views))
    }

    /// A snapshot of `views` already read.
    pub fn of(base: &str, views: Vec<PointInTime>) -> Self {
        Self {
            // ALLOC: the base, once a pass.
            base: base.to_owned(),
            views,
            answered: Mutex::new(HashSet::new()),
        }
    }
}

impl Pins for PointInTimePins {
    /// What the views hold of `shard`. A view that cannot be read, or that
    /// is a capture of the shard's index not finished yet, pins every blob;
    /// so does one whose file locations cannot be followed, since what it
    /// holds is unknowable. No request is sent. A shard asked about a second
    /// time gets [`Pinned::Everything`]: the snapshot is no later than the
    /// sweep that asked first, and a deletion needs a later one.
    fn pinned(&self, shard: &ShardRef<'_>) -> Result<Pinned, JanitorError> {
        let path = shard.base_path(&self.base)?;
        {
            let mut answered = self.answered.lock().unwrap_or_else(|e| e.into_inner());
            // ALLOC: one path a shard of the pass, at most the pass's shards.
            if !answered.insert(path) {
                return Ok(Pinned::Everything);
            }
        }
        // ALLOC: one key a file a view names of this shard, bounded by
        // the collector's max_pinned, which refuses more.
        let mut keys: Vec<String> = Vec::new();
        for view in &self.views {
            if view.is_placeholder() {
                return Ok(Pinned::Everything);
            }
            // A capture holds the whole index until it finishes, whatever
            // incarnation it names, as the Java's `pins(index, null)`.
            if view.capturing && view.pins(shard.index, None) {
                return Ok(Pinned::Everything);
            }
            if !view.pins(shard.index, Some(shard.uuid)) {
                continue;
            }
            for file in view
                .shards
                .iter()
                .filter(|s| s.shard == shard.shard)
                .flat_map(|s| &s.files)
            {
                let Ok(location) = FileLocation::parse(&file.location) else {
                    return Ok(Pinned::Everything);
                };
                let Ok((key, _)) = shard.blob_key(&self.base, &file.name, &location) else {
                    return Ok(Pinned::Everything);
                };
                keys.push(key);
            }
        }
        Ok(Pinned::Blobs(keys))
    }
}
