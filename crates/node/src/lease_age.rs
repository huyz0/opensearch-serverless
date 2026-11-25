//! How long ago this node last renewed its own lease, by its own clock.
//!
//! A node answers a forwarded search of a shard it writes only while its
//! lease is fresh, so a node that has missed renewals refuses rather than
//! answers from a view a successor may have passed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Marks of the lease thread's successful renewals, read by the handlers
/// that must not answer from a stale lease.
#[derive(Debug)]
pub struct LeaseAge {
    epoch: Instant,
    /// Milliseconds after `epoch` of the last renewal, plus one; 0 for none.
    last: AtomicU64,
}

impl Default for LeaseAge {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            last: AtomicU64::new(0),
        }
    }
}

impl LeaseAge {
    /// Notes a renewal that just succeeded.
    pub fn mark(&self) {
        self.mark_at(Instant::now());
    }

    fn mark_at(&self, now: Instant) {
        let millis = u64::try_from(now.saturating_duration_since(self.epoch).as_millis())
            .unwrap_or(u64::MAX);
        self.last.store(millis.saturating_add(1), Ordering::Release);
    }

    /// Milliseconds since the last renewal; `None` before the first.
    pub fn millis(&self) -> Option<u64> {
        self.millis_at(Instant::now())
    }

    fn millis_at(&self, now: Instant) -> Option<u64> {
        let last = self.last.load(Ordering::Acquire).checked_sub(1)?;
        let at = u64::try_from(now.saturating_duration_since(self.epoch).as_millis())
            .unwrap_or(u64::MAX);
        Some(at.saturating_sub(last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn no_renewal_has_no_age_and_a_renewal_has_the_time_since_it() {
        let age = LeaseAge::default();
        let start = age.epoch;
        assert_eq!(age.millis_at(start), None);
        age.mark_at(start + Duration::from_millis(40));
        assert_eq!(age.millis_at(start + Duration::from_millis(40)), Some(0));
        assert_eq!(
            age.millis_at(start + Duration::from_millis(1040)),
            Some(1000)
        );
        age.mark_at(start + Duration::from_millis(2000));
        assert_eq!(
            age.millis_at(start + Duration::from_millis(2500)),
            Some(500)
        );
    }

    #[test]
    fn the_real_clock_marks_and_reads() {
        let age = LeaseAge::default();
        age.mark();
        assert!(age.millis().is_some_and(|m| m < 60_000));
    }
}
