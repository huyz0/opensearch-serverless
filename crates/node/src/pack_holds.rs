//! The frozen views that hold a node pack: the live points in time and the
//! snapshots, as the janitor reads them.
//!
//! A view holds its index incarnation's packs whole, as it holds the
//! incarnation's blobs against the janitor, so a pack is judged named while
//! any live view names an incarnation with a block in it. The records are
//! read once a pass, on the first question, and every later question of
//! the pass is answered from memory: a pass that has no pack due reads
//! nothing, and a view made after a pack was first seen unreferenced is in
//! the read that deletes it, because each pass reads afresh.

use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::BlobStore;
use oss_janitor::JanitorError;
use oss_janitor::orphans::{IndexPins, PinSource, PointInTimeSource, SnapshotSource};
use oss_meta::PointInTimeStore;
use oss_writer::pack::{IndexHolds, PackError};

/// The bounds on what one pass reads of the views.
#[derive(Clone, Copy, Debug)]
pub struct HoldLimits {
    /// The most points in time or snapshots a pass reads, each.
    pub max_records: usize,
    /// The most incarnations the views may hold.
    pub max_pins: usize,
    /// The longest snapshot record read.
    pub record_bytes: usize,
}

/// One pass's view of the incarnations held, read on first use.
pub struct PassHolds {
    pits: Arc<PointInTimeStore>,
    snapshots: SnapshotSource,
    limits: HoldLimits,
    pins: Mutex<Option<IndexPins>>,
}

impl PassHolds {
    /// A pass over the views under `base`; nothing is read yet.
    pub fn new(
        pits: Arc<PointInTimeStore>,
        store: Arc<dyn BlobStore>,
        base: &str,
        limits: HoldLimits,
    ) -> Self {
        Self {
            pits,
            snapshots: SnapshotSource::new(store, base, limits.max_records, limits.record_bytes),
            limits,
            pins: Mutex::new(None),
        }
    }

    /// Cost: one LIST page per 1,000 views and a register read each, then
    /// one LIST page per 1,000 snapshots and a GET each.
    fn read(&self) -> Result<IndexPins, PackError> {
        let mut pins = IndexPins::new(self.limits.max_pins);
        let points = PointInTimeSource::new(&self.pits, self.limits.max_records);
        points
            .collect(&mut pins)
            .and_then(|()| self.snapshots.collect(&mut pins))
            .map_err(|e| match e {
                JanitorError::Blob(b) => PackError::Blob(b),
                // Unreadable is not "none": the pass stops.
                _ => PackError::Unreadable("a point in time or a snapshot could not be read"),
            })?;
        Ok(pins)
    }
}

impl IndexHolds for PassHolds {
    fn holds(&self, index: &str, uuid: &str) -> Result<bool, PackError> {
        let mut slot = self.pins.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(self.read()?);
        }
        Ok(slot.as_ref().is_some_and(|p| p.contains(index, uuid)))
    }
}
