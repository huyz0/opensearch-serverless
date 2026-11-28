//! The arithmetic of a detector's schedule: which slot is due at a time and the
//! window a slot reads.
//!
//! A slot is a whole number of periods since the epoch: slot `s` covers
//! `[s * period, (s + 1) * period)`. Every node computes the same slot from
//! its own clock, so a skewed clock changes *when* a node tries a slot and
//! never *which* slot or window it means; the claim register decides who
//! runs it. A node runs a slot only after its end plus `max_skew` plus
//! `visibility_lag`, so a node whose clock is fast by up to `max_skew` still
//! reads a window that is complete and visible.

/// How long after a slot's end a run waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// The most two nodes' clocks may differ, in milliseconds.
    pub max_skew_ms: i64,
    /// How long a written document takes to be searchable, in milliseconds.
    pub visibility_lag_ms: i64,
}

impl Timing {
    /// 5 seconds of skew and 2 of visibility.
    pub const DEFAULT: Self = Self {
        max_skew_ms: 5_000,
        visibility_lag_ms: 2_000,
    };

    fn wait_ms(self) -> i64 {
        self.max_skew_ms
            .max(0)
            .saturating_add(self.visibility_lag_ms.max(0))
    }
}

/// The newest slot whose end, plus the wait, has passed at `now_ms`, or
/// `None` before the first slot is due or for a period that is not positive.
/// A slot is never run late to catch up: only this one is offered.
#[must_use]
pub fn latest_due(now_ms: i64, period_ms: i64, timing: Timing) -> Option<u64> {
    if period_ms <= 0 {
        return None;
    }
    let ready = now_ms.checked_sub(timing.wait_ms())?;
    let ended = ready.checked_div(period_ms)?;
    // `ended` slots have ended, so the last one that has is `ended - 1`.
    u64::try_from(ended.checked_sub(1)?).ok()
}

/// The data window of `slot` in epoch milliseconds: it ends `delay_ms`
/// before the slot's end and is one period long. `None` when the numbers
/// leave `i64`.
#[must_use]
pub fn window(slot: u64, period_ms: i64, delay_ms: i64) -> Option<(i64, i64)> {
    let end_of_slot = i64::try_from(slot.checked_add(1)?)
        .ok()?
        .checked_mul(period_ms)?;
    let end = end_of_slot.checked_sub(delay_ms)?;
    Some((end.checked_sub(period_ms)?, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;
    const T: Timing = Timing {
        max_skew_ms: 5_000,
        visibility_lag_ms: 2_000,
    };

    #[test]
    fn a_slot_is_due_only_after_its_end_plus_the_wait() {
        // slot 2 ends at 3 minutes; due from 3 minutes and 7 seconds
        assert_eq!(latest_due(3 * MIN + 6_999, MIN, T), Some(1));
        assert_eq!(latest_due(3 * MIN + 7_000, MIN, T), Some(2));
        assert_eq!(latest_due(3 * MIN + 7_001, MIN, T), Some(2));
    }

    #[test]
    fn nothing_is_due_before_the_first_slot_ends() {
        assert_eq!(latest_due(0, MIN, T), None);
        assert_eq!(latest_due(MIN + 6_999, MIN, T), None);
        assert_eq!(latest_due(MIN + 7_000, MIN, T), Some(0));
        assert_eq!(latest_due(-5, MIN, T), None);
    }

    #[test]
    fn a_long_gap_offers_only_the_newest_slot() {
        assert_eq!(latest_due(100 * MIN + 7_000, MIN, T), Some(99));
    }

    #[test]
    fn a_skew_inside_the_bound_never_picks_a_slot_that_has_not_ended() {
        // A clock fast by the whole bound may pick a slot one wait earlier than
        // the true clock would, but that slot has ended on the true clock.
        for step in 0..600 {
            let true_now = 3 * MIN + step * 1_000;
            let fast = true_now + T.max_skew_ms;
            if let Some(slot) = latest_due(fast, MIN, T) {
                let end = (i64::try_from(slot).unwrap() + 1) * MIN;
                assert!(
                    end <= true_now,
                    "slot {slot} ends at {end}, true time {true_now}"
                );
            }
        }
    }

    #[test]
    fn a_period_that_is_not_positive_is_never_due() {
        assert_eq!(latest_due(10 * MIN, 0, T), None);
        assert_eq!(latest_due(10 * MIN, -MIN, T), None);
    }

    #[test]
    fn a_window_is_the_slot_less_the_delay() {
        assert_eq!(window(2, MIN, 0), Some((2 * MIN, 3 * MIN)));
        assert_eq!(
            window(2, MIN, 10_000),
            Some((2 * MIN - 10_000, 3 * MIN - 10_000))
        );
        assert_eq!(window(u64::MAX, MIN, 0), None);
        assert_eq!(window(i64::MAX as u64, MIN, 0), None);
    }
}
