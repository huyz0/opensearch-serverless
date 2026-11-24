//! Deleting a snapshot: releasing the name's register and removing the
//! lifetime's discovery marker, as the shell's `SnapshotStore.remove` and
//! `SnapshotRecords.deleteSnapshot` do for a shallow snapshot.
//!
//! A shallow snapshot copies no data, so a delete reclaims no data blobs:
//! the register is released first (the janitor stops pinning the indices),
//! then the marker is deleted. A marker whose register is released is
//! skipped by every reader, so a failure between the two steps leaves
//! nothing that names storage.

use oss_format::{SnapshotState, SnapshotSummary, snapshot_state_key};

use crate::snapshot_take::{ATTEMPTS, SnapshotTaker, TakeError, state};
use oss_blob::SwapOutcome;

impl SnapshotTaker {
    /// Deletes the snapshot `repo`/`name`; whether there was one. A snapshot
    /// still being taken is deleted too (its take then finds the capture no
    /// longer active and cleans up after itself). `value` is the buffer the
    /// register is read into.
    ///
    /// Cost: a register GET, a swap and a marker DELETE; no LIST.
    ///
    /// # Errors
    ///
    /// [`TakeError::NotShallow`] for a snapshot that copied its data;
    /// [`TakeError::Conflict`] after 16 lost swaps; [`TakeError`] otherwise.
    pub fn remove(&self, repo: &str, name: &str, value: &mut [u8]) -> Result<bool, TakeError> {
        let state_key = snapshot_state_key(&self.base, repo, name)?;
        for _ in 0..ATTEMPTS {
            let Some(read) = self.read(&state_key, value)? else {
                return Ok(false);
            };
            let marker = match state(value, read)? {
                SnapshotState::Released => return Ok(false),
                SnapshotState::Held { marker, record, .. } => {
                    if !SnapshotSummary::parse(record)?.shallow {
                        return Err(TakeError::NotShallow);
                    }
                    // ALLOC: one marker name, about 160 bytes, once a delete.
                    marker.to_owned()
                }
            };
            let released = self.store.swap_register(
                &state_key,
                read.generation,
                SnapshotState::released_value(),
            )?;
            if matches!(released, SwapOutcome::Applied(_)) {
                // ALLOC: one key, once a delete.
                let marker_key = [self.base.as_str(), "snapshots/", &marker].concat();
                self.store.delete(&marker_key)?;
                return Ok(true);
            }
        }
        Err(TakeError::Conflict(
            "snapshot deletion conflicted repeatedly",
        ))
    }
}
