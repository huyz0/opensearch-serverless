//! When the S3 client tries a request again, and how long it waits first.
//!
//! The Java shell leaves this to the AWS SDK's retry strategy, which hides
//! its attempts from the meter. Here the policy is ours (ADR 0005): capped
//! exponential backoff with full jitter, a fixed number of attempts, and a
//! classification of each failure as worth retrying or not. Every attempt
//! is recorded by the caller of [`RetryPolicy::pause`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::error::BlobError;

/// How many times a request is attempted and how long to wait between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Attempts per call, the first included; at least 1.
    pub max_attempts: u32,
    /// The pause bound before the first retry; it doubles per retry.
    pub base: Duration,
    /// The largest pause bound, however many retries came before.
    pub cap: Duration,
}

impl Default for RetryPolicy {
    /// Four attempts, pauses bounded by 25 ms, 50 ms and 100 ms: about the
    /// latency of one S3 request each (ADR 0005), so a retried call costs a
    /// few round trips, not seconds.
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base: Duration::from_millis(25),
            cap: Duration::from_secs(1),
        }
    }
}

impl RetryPolicy {
    /// The bound on the pause before retry number `retry` (0 for the first
    /// retry): `min(cap, base * 2^retry)`.
    pub fn bound(&self, retry: u32) -> Duration {
        let factor = 1u32.checked_shl(retry).unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.cap)
    }

    /// The pause before retry number `retry`: uniform in `[0, bound]` by
    /// `roll`, the full jitter that spreads retrying clients apart.
    pub fn pause(&self, retry: u32, roll: u64) -> Duration {
        let bound = u64::try_from(self.bound(retry).as_nanos()).unwrap_or(u64::MAX);
        // A span of u64::MAX + 1 nanoseconds takes every roll as it is.
        let nanos = bound
            .checked_add(1)
            .and_then(|span| roll.checked_rem(span))
            .unwrap_or(roll);
        Duration::from_nanos(nanos)
    }
}

/// Whether a failed attempt is worth repeating as it was.
pub fn retryable(err: &BlobError) -> bool {
    matches!(
        err,
        BlobError::Throttled | BlobError::Unavailable | BlobError::Conflict
    )
}

/// A lock-free source of jitter rolls (SplitMix64), shared by every thread
/// using one store.
#[derive(Debug)]
pub struct Jitter {
    state: AtomicU64,
}

impl Jitter {
    /// A generator started from `seed`.
    pub fn new(seed: u64) -> Self {
        Self {
            state: AtomicU64::new(seed),
        }
    }

    /// The next roll.
    pub fn roll(&self) -> u64 {
        const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut z = self
            .state
            .fetch_add(GAMMA, Ordering::Relaxed)
            .wrapping_add(GAMMA);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bound_doubles_up_to_the_cap() {
        let p = RetryPolicy {
            max_attempts: 9,
            base: Duration::from_millis(10),
            cap: Duration::from_millis(70),
        };
        let bounds: Vec<u128> = (0..5).map(|r| p.bound(r).as_millis()).collect();
        assert_eq!(bounds, [10, 20, 40, 70, 70]);
        assert_eq!(p.bound(40), Duration::from_millis(70));
    }

    proptest::proptest! {
        /// Every pause lies in [0, min(cap, base * 2^retry)].
        #[test]
        fn every_pause_lies_within_its_bound(retry in 0u32..80, roll: u64, base_ms in 0u64..500, cap_ms in 0u64..5000) {
            let p = RetryPolicy { max_attempts: 4, base: Duration::from_millis(base_ms), cap: Duration::from_millis(cap_ms) };
            proptest::prop_assert!(p.pause(retry, roll) <= p.bound(retry));
        }
    }

    #[test]
    fn rolls_spread_pauses_across_the_bound() {
        let p = RetryPolicy::default();
        let j = Jitter::new(7);
        let pauses: Vec<Duration> = (0..64).map(|_| p.pause(2, j.roll())).collect();
        let bound = p.bound(2);
        assert!(pauses.iter().any(|d| *d < bound / 4));
        assert!(pauses.iter().any(|d| *d > bound / 2));
    }

    #[test]
    fn a_bound_of_u64_max_nanos_uses_the_roll_as_is() {
        let p = RetryPolicy {
            max_attempts: 2,
            base: Duration::MAX,
            cap: Duration::MAX,
        };
        assert_eq!(p.pause(0, 12345), Duration::from_nanos(12345));
    }

    #[test]
    fn only_transient_failures_are_retried() {
        for err in [
            BlobError::Throttled,
            BlobError::Unavailable,
            BlobError::Conflict,
        ] {
            assert!(retryable(&err), "{err:?}");
        }
        for err in [
            BlobError::NotFound,
            BlobError::InvalidRange,
            BlobError::Denied,
            BlobError::Unexpected(400),
            BlobError::Protocol("x"),
            BlobError::Stream("x"),
        ] {
            assert!(!retryable(&err), "{err:?}");
        }
    }
}
