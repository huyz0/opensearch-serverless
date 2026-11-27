//! The most bytes a second a peer serves to joining nodes, across all of them.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A peer's serve rate: each page batch reserves the time its bytes take
/// at the rate, after the batches already reserved.
#[derive(Debug)]
pub struct ServeRate {
    bytes_per_second: u64,
    next_free: Mutex<Option<Instant>>,
}

impl ServeRate {
    /// No limit.
    #[must_use]
    pub fn unlimited() -> Self {
        Self::new(0)
    }

    /// At most `bytes_per_second`; 0 is no limit.
    #[must_use]
    pub fn new(bytes_per_second: u64) -> Self {
        Self {
            bytes_per_second,
            next_free: Mutex::new(None),
        }
    }

    /// How long a batch of `bytes` asked for at `now` waits before it is sent.
    ///
    /// The batch is sent once those reserved before it have been, and the
    /// next one is held for the time these bytes take. A peer idle past its
    /// reservations has no credit: the first batch after is free, the next
    /// waits for it.
    pub fn reserve(&self, now: Instant, bytes: u64) -> Duration {
        if self.bytes_per_second == 0 {
            return Duration::ZERO;
        }
        let mut next = self
            .next_free
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let start = next.map_or(now, |free| free.max(now));
        let took = Duration::from_nanos(
            bytes
                .saturating_mul(1_000_000_000)
                .checked_div(self.bytes_per_second)
                .unwrap_or(u64::MAX),
        );
        *next = start.checked_add(took).or(Some(start));
        start.saturating_duration_since(now)
    }
}

impl Default for ServeRate {
    fn default() -> Self {
        Self::unlimited()
    }
}

#[cfg(test)]
mod tests;
