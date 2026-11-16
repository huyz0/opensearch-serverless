//! Closing an epoch to its writer: a fence where the writer must stop.

use oss_format::NodeLogEntry;
use oss_format::node_log_format::encode_fence;

use super::store::create;
use super::{FENCE_ATTEMPTS, LogError, MAX_FENCE_GAP, NodeLogStore};

impl NodeLogStore {
    /// Puts a fence where the epoch's writer must stop, and returns the
    /// lowest fence in the epoch: every slot below it holds one of the
    /// writer's blobs.
    ///
    /// `from` must not be past the first free slot; the landed hint the
    /// node's lease records always qualifies, and a hint seconds old costs
    /// a handful of HEADs.
    ///
    /// A writer appending back to back is always a request ahead of a
    /// fence aimed at the first free slot. So after a lost race the fence
    /// aims ahead of it, by a gap that doubles up to [`MAX_FENCE_GAP`]. The
    /// writer never skips a slot, so it stops at that barrier at the
    /// latest; the fence then fills the gap from the bottom, and the first
    /// fence it lands there is where the writer stopped. A fence another
    /// successor already placed is shared rather than added to.
    ///
    /// # Errors
    ///
    /// [`LogError::Blob`] or [`LogError::Format`] if the store fails or a
    /// slot is malformed; [`LogError::FenceExhausted`] if the writer kept
    /// taking the next slot.
    pub fn fence(
        &self,
        epoch: u64,
        from: u64,
        fenced_by: &str,
        term: i64,
    ) -> Result<u64, LogError> {
        let fence = encode_fence(fenced_by, term)?;
        let mut slot = self.first_free(epoch, from)?;
        let mut gap = 0u64;
        for _ in 0..FENCE_ATTEMPTS {
            // A fence just below the first free slot closed the epoch
            // already: another successor's, which this one shares.
            if let Some(below) = slot.checked_sub(1)
                && self.holds_fence(epoch, below)?
            {
                return Ok(below);
            }
            let target = slot.checked_add(gap).ok_or(LogError::OutOfRange("slot"))?;
            // Placed here, or taken first by another successor's fence.
            if self.try_fence(epoch, target, &fence)? || self.holds_fence(epoch, target)? {
                return if gap == 0 {
                    Ok(target)
                } else {
                    self.fill_below(epoch, slot, target, &fence)
                };
            }
            // The writer took it first.
            let next = target.checked_add(1).ok_or(LogError::OutOfRange("slot"))?;
            slot = self.first_free(epoch, next)?;
            gap = if gap == 0 {
                1
            } else {
                gap.saturating_mul(2).min(MAX_FENCE_GAP)
            };
        }
        Err(LogError::FenceExhausted)
    }

    /// With a barrier fence at `barrier`, fences the lowest free slot from
    /// `from` up, and returns the lowest fence at or below the barrier:
    /// where the writer stopped.
    fn fill_below(
        &self,
        epoch: u64,
        from: u64,
        barrier: u64,
        fence: &[u8],
    ) -> Result<u64, LogError> {
        for slot in from..barrier {
            if self.try_fence(epoch, slot, fence)? || self.holds_fence(epoch, slot)? {
                return Ok(slot);
            }
        }
        Ok(barrier)
    }

    fn try_fence(&self, epoch: u64, slot: u64, fence: &[u8]) -> Result<bool, LogError> {
        match create(self.store.as_ref(), &self.key(epoch, slot)?, fence) {
            Ok(()) => Ok(true),
            Err(LogError::Taken) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn holds_fence(&self, epoch: u64, slot: u64) -> Result<bool, LogError> {
        Ok(matches!(
            self.read_entry(epoch, slot)?,
            Some(NodeLogEntry::Fence(_))
        ))
    }
}
