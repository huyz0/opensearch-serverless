//! The write limiter's place in the writer role: admitting a group before
//! anything is applied, and telling the limiter how the log append went.

use oss_writer::admission::{Permit, WriteLimiter};
use oss_writer::index::Written;

use super::{Failure, WriterRole};

impl WriterRole {
    /// Limits the writes this role lets wait on the log with `limiter`.
    /// False when one is set already.
    pub fn set_write_limiter(&self, limiter: WriteLimiter) -> bool {
        self.limiter.set(limiter).is_ok()
    }

    /// Nanoseconds since this role started: the limiter's clock.
    pub(super) fn limiter_now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    /// A slot for a group of `items`, or the outcomes of a group refused:
    /// each item fails with the limiter's reason, nothing applied or logged.
    /// A shard that was asleep is woken and admitted again, so that write
    /// counts twice while it wakes.
    pub(super) fn admit_group(
        &self,
        items: usize,
    ) -> Result<Option<Permit>, Vec<Result<Written, Failure>>> {
        let Some(limiter) = self.limiter.get() else {
            return Ok(None);
        };
        limiter.admit(self.limiter_now()).map(Some).map_err(|t| {
            (0..items)
                // ALLOC: one outcome and one message an item of a refused group.
                .map(|_| Err(Failure::Throttled(t.reason.clone())))
                .collect()
        })
    }

    /// Tells the limiter a log append begun at `began` landed or failed.
    pub(super) fn appended(&self, began: u64, landed: bool) {
        if let Some(limiter) = self.limiter.get() {
            let now = self.limiter_now();
            limiter.on_append(now, now.saturating_sub(began), landed);
        }
    }
}
