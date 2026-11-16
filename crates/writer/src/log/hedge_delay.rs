//! How long a node-log PUT waits before it is sent a second time.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// How many recent PUTs the delay is drawn from.
const RECENT_PUTS: usize = 128;

/// The share of PUTs, at most, that hedging sends twice: the delay is at
/// least this quantile of the recent ones (95th, as `HEDGE_QUANTILE`).
const QUANTILE_PERCENT: usize = 95;

/// The configured delay, raised to the recent PUTs' tail and scaled by a
/// blob's size (the Java's `NodeLogHedgeDelay`).
#[derive(Debug, Default)]
pub(crate) struct HedgeDelay {
    inner: Mutex<Recent>,
}

#[derive(Debug)]
struct Recent {
    configured: Duration,
    took: [Duration; RECENT_PUTS],
    bytes: [u64; RECENT_PUTS],
    seen: usize,
}

impl Default for Recent {
    fn default() -> Self {
        Self {
            configured: Duration::ZERO,
            took: [Duration::ZERO; RECENT_PUTS],
            bytes: [0; RECENT_PUTS],
            seen: 0,
        }
    }
}

impl HedgeDelay {
    fn recent(&self) -> std::sync::MutexGuard<'_, Recent> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets how long a PUT may go unanswered before it is sent again; zero
    /// never sends a second.
    pub(crate) fn configure(&self, after: Duration) {
        self.recent().configured = after;
    }

    /// Remembers how long a PUT that landed took, and how big it was.
    pub(crate) fn note_put_took(&self, took: Duration, bytes: u64) {
        let mut recent = self.recent();
        let at = recent.seen.checked_rem(RECENT_PUTS).unwrap_or(0);
        if let Some(t) = recent.took.get_mut(at) {
            *t = took;
        }
        if let Some(b) = recent.bytes.get_mut(at) {
            *b = bytes;
        }
        recent.seen = recent.seen.saturating_add(1);
    }

    /// How long a PUT of `blob_bytes` waits before it is sent again: the
    /// configured delay, or the 95th percentile of the recent PUTs when
    /// longer; then scaled up by how many times the recent PUTs' median
    /// size the blob is. Zero when hedging is off.
    pub(crate) fn delay(&self, blob_bytes: u64) -> Duration {
        let (configured, mut took, mut sizes, n) = {
            let recent = self.recent();
            let n = recent.seen.min(RECENT_PUTS);
            (recent.configured, recent.took, recent.bytes, n)
        };
        if configured.is_zero() {
            return Duration::ZERO;
        }
        let (Some(took), Some(sizes)) = (took.get_mut(..n), sizes.get_mut(..n)) else {
            return configured;
        };
        if took.is_empty() {
            return configured;
        }
        took.sort_unstable();
        let at = QUANTILE_PERCENT
            .saturating_mul(took.len())
            .checked_div(100)
            .unwrap_or(0)
            .min(took.len().saturating_sub(1));
        let delay = took.get(at).copied().unwrap_or_default().max(configured);
        if blob_bytes == 0 {
            return delay;
        }
        sizes.sort_unstable();
        let median = sizes
            .get(sizes.len().checked_div(2).unwrap_or(0))
            .copied()
            .unwrap_or(1)
            .max(1);
        if blob_bytes <= median {
            return delay;
        }
        // Scaled in nanoseconds as a ratio, capped where the Java caps it.
        let scaled = u128::from(blob_bytes)
            .saturating_mul(delay.as_nanos())
            .checked_div(u128::from(median))
            .unwrap_or(u128::MAX);
        let cap = u128::from(u64::MAX.checked_div(4).unwrap_or(0));
        Duration::from_nanos(u64::try_from(scaled.min(cap)).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn hedge_delay_is_off_at_zero_and_the_setting_until_puts_are_seen() {
        let d = HedgeDelay::default();
        assert_eq!(d.delay(100), Duration::ZERO);
        d.note_put_took(ms(500), 100);
        assert_eq!(
            d.delay(100),
            Duration::ZERO,
            "zero is off whatever was seen"
        );
        let d = HedgeDelay::default();
        d.configure(ms(60));
        assert_eq!(d.delay(100), ms(60));
    }

    #[test]
    fn hedge_delay_is_at_least_the_95th_percentile_of_recent_puts() {
        let d = HedgeDelay::default();
        d.configure(ms(60));
        // 100 PUTs of 1..=100 ms: the 95th is index 95, 96 ms.
        for n in 1..=100 {
            d.note_put_took(ms(n), 1000);
        }
        assert_eq!(d.delay(1000), ms(96));
        assert_eq!(d.delay(0), ms(96), "no size scales nothing");
        // Fast PUTs leave the setting in charge.
        let fast = HedgeDelay::default();
        fast.configure(ms(60));
        for _ in 0..200 {
            fast.note_put_took(ms(5), 1000);
        }
        assert_eq!(fast.delay(1000), ms(60));
    }

    #[test]
    fn hedge_delay_remembers_only_the_last_128_puts() {
        let d = HedgeDelay::default();
        d.configure(ms(1));
        for _ in 0..128 {
            d.note_put_took(ms(900), 10);
        }
        for _ in 0..128 {
            d.note_put_took(ms(10), 10);
        }
        assert_eq!(d.delay(10), ms(10), "the slow ones have rolled off");
    }

    #[test]
    fn hedge_delay_scales_a_blob_larger_than_the_median() {
        let d = HedgeDelay::default();
        d.configure(ms(60));
        for _ in 0..10 {
            d.note_put_took(ms(10), 1000);
        }
        assert_eq!(d.delay(1000), ms(60), "at the median: unscaled");
        assert_eq!(d.delay(500), ms(60), "below it: unscaled");
        assert_eq!(
            d.delay(10_000),
            ms(600),
            "ten times the median: ten times the delay"
        );
        assert_eq!(
            d.delay(u64::MAX),
            Duration::from_nanos(u64::MAX / 4),
            "capped as the Java caps it"
        );
    }
}
