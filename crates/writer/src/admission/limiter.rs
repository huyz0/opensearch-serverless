//! The write limiter: additive increase, multiplicative decrease on the
//! writes in flight, driven by how the object store answers log appends.
//!
//! Port of the shell's `WriteBackpressure`. A write is admitted or refused
//! before anything is applied, so a refused write is in neither the engine
//! nor the log, and an admitted one is acknowledged only once its record
//! has landed.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

const MILLI: u64 = 1_000_000;
/// At most one cut in this long, in nanoseconds.
const CUT_EVERY: u64 = 500 * MILLI;
const SECOND: u64 = 1_000 * MILLI;
const MAX_RETRY_AFTER: u64 = 30;

/// Why a write was refused, with the hint a client acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Throttled {
    /// How long to wait before trying again, in seconds: 1 to 30.
    pub retry_after_secs: u64,
    /// The refusal in words, for the 429's body.
    pub reason: String,
}

impl fmt::Display for Throttled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for Throttled {}

/// The wait a pressure of `pressure` nanoseconds asks of a client, in whole
/// seconds plus one, between 1 and 30.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn retry_after_of(pressure: f64) -> u64 {
    ((pressure as u64) / SECOND)
        .saturating_add(1)
        .clamp(1, MAX_RETRY_AFTER)
}

/// What the limiter is doing now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    /// The most writes it lets wait now.
    pub limit: usize,
    /// Writes admitted and not yet released.
    pub in_flight: usize,
    /// The smoothed append time, in milliseconds.
    pub append_millis: u64,
    /// Writes refused so far.
    pub refused: u64,
}

#[derive(Debug)]
struct State {
    limit: f64,
    latency: f64,
    last_cut: Option<u64>,
    /// Admitted writes: admission order to the time each was admitted. The
    /// first is the oldest.
    waiting: BTreeMap<u64, u64>,
    admissions: u64,
    refused: u64,
}

#[derive(Debug)]
struct Inner {
    target: u64,
    budget: u64,
    min: usize,
    max: usize,
    state: Mutex<State>,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        // Every update is a few steps that cannot panic, so a poisoned lock
        // still holds consistent counts.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Cuts the limit by a third, at most once in [`CUT_EVERY`].
    fn slow(&self, s: &mut State, now: u64) {
        if s.last_cut
            .is_none_or(|at| now.saturating_sub(at) > CUT_EVERY)
        {
            #[allow(clippy::cast_precision_loss)]
            let floor = self.min as f64;
            s.limit = floor.max(s.limit * (2.0 / 3.0));
            s.last_cut = Some(now);
        }
    }
}

/// What a limiter is built from: the values of the
/// `serverless.write.backpressure.*` settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimiterConfig {
    /// An append slower than this, in milliseconds, is a sign to back off.
    pub target_millis: u64,
    /// While appends take longer than this, writes beyond the minimum wait.
    pub budget_millis: u64,
    /// The fewest writes let through at once.
    pub min_in_flight: usize,
    /// The most.
    pub max_in_flight: usize,
}

impl Default for LimiterConfig {
    fn default() -> Self {
        Self {
            target_millis: 1_000,
            budget_millis: 5_000,
            min_in_flight: 4,
            max_in_flight: 1_024,
        }
    }
}

impl LimiterConfig {
    /// A limiter of these values.
    pub fn build(&self) -> WriteLimiter {
        WriteLimiter::new(
            self.target_millis,
            self.budget_millis,
            self.min_in_flight,
            self.max_in_flight,
        )
    }
}

/// A slot for one admitted write, given back on drop.
#[derive(Debug)]
pub struct Permit {
    inner: Arc<Inner>,
    key: u64,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.inner.lock().waiting.remove(&self.key);
    }
}

/// How many writes a node lets wait on the store at once.
#[derive(Clone, Debug)]
pub struct WriteLimiter {
    inner: Arc<Inner>,
}

impl WriteLimiter {
    /// A limiter: an append slower than `target_millis` is a sign to back
    /// off; while appends take longer than `budget_millis` (raised to at
    /// least the target), writes beyond `min` in flight are refused; the
    /// limit stays between `min` (at least 1) and `max`, and starts at `max`.
    pub fn new(target_millis: u64, budget_millis: u64, min: usize, max: usize) -> Self {
        let target = target_millis.max(1).saturating_mul(MILLI);
        let budget = budget_millis.max(target_millis).saturating_mul(MILLI);
        let min = min.max(1);
        let max = max.max(min);
        #[allow(clippy::cast_precision_loss)]
        let limit = max as f64;
        Self {
            inner: Arc::new(Inner {
                target,
                budget,
                min,
                max,
                state: Mutex::new(State {
                    limit,
                    latency: 0.0,
                    last_cut: None,
                    // ALLOC: one entry a write in flight, at most the limit.
                    waiting: BTreeMap::new(),
                    admissions: 0,
                    refused: 0,
                }),
            }),
        }
    }

    /// Admits one write at `now` (nanoseconds on any steady clock), or
    /// refuses it.
    ///
    /// # Errors
    ///
    /// [`Throttled`] when the store is too slow to take it now.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    pub fn admit(&self, now: u64) -> Result<Permit, Throttled> {
        let inner = &self.inner;
        let mut s = inner.lock();
        // The oldest write still waiting, as well as how long appends took:
        // an append is only measured once it ends, and a store answering
        // SlowDown keeps appends in retries for tens of seconds.
        let oldest_wait = s
            .waiting
            .first_key_value()
            .map_or(0, |(_, at)| now.saturating_sub(*at));
        if oldest_wait > inner.target {
            inner.slow(&mut s, now);
        }
        let limit = s.limit.floor() as usize;
        let pressure = s.latency.max(oldest_wait as f64);
        let current = s.waiting.len().saturating_add(1);
        if current > limit || (pressure > inner.budget as f64 && current > inner.min) {
            s.refused = s.refused.saturating_add(1);
            let retry_after_secs = retry_after_of(pressure);
            return Err(Throttled {
                retry_after_secs,
                // ALLOC: error path; one message.
                reason: format!(
                    "the object store is slow to take writes (an append is taking about {} ms; \
                     {} writes already waiting, limit {limit}); retry after {retry_after_secs} s",
                    (pressure as u64) / MILLI,
                    current.saturating_sub(1),
                ),
            });
        }
        s.admissions = s.admissions.saturating_add(1);
        let key = s.admissions;
        s.waiting.insert(key, now);
        drop(s);
        Ok(Permit {
            inner: Arc::clone(&self.inner),
            key,
        })
    }

    /// What a refusal at `now` would tell a client to wait, in seconds, 1 to
    /// 30: the pressure in whole seconds plus one.
    #[allow(clippy::cast_precision_loss)]
    pub fn retry_after(&self, now: u64) -> u64 {
        let s = self.inner.lock();
        let oldest_wait = s
            .waiting
            .first_key_value()
            .map_or(0, |(_, at)| now.saturating_sub(*at));
        retry_after_of(s.latency.max(oldest_wait as f64))
    }

    /// Records how one log append went: it took `elapsed` nanoseconds and
    /// did or did not land, ending at `now`.
    #[allow(clippy::cast_precision_loss)]
    pub fn on_append(&self, now: u64, elapsed: u64, succeeded: bool) {
        let inner = &self.inner;
        let mut s = inner.lock();
        let sample = elapsed as f64;
        s.latency = if s.latency == 0.0 {
            sample
        } else {
            0.8 * s.latency + 0.2 * sample
        };
        let target = inner.target as f64;
        let max = inner.max as f64;
        // On the smoothed time, not each sample: a store answering in 300
        // to 600 ms with the odd append past the target must not be cut by
        // a third twice a second.
        if !succeeded || s.latency > target {
            inner.slow(&mut s, now);
        } else if s.latency < target / 2.0 {
            // Comfortably inside the target: grow geometrically; additive
            // growth alone takes an hour to climb back after a burst.
            s.limit = max.min(s.limit * 1.02 + 0.5);
        } else {
            s.limit = max.min(s.limit + 1.0 / s.limit.max(1.0) * 4.0);
        }
    }

    /// The current limit, writes in flight, smoothed append time and
    /// refusals so far.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn stats(&self) -> Stats {
        let s = self.inner.lock();
        Stats {
            limit: s.limit.floor() as usize,
            in_flight: s.waiting.len(),
            append_millis: (s.latency as u64) / MILLI,
            refused: s.refused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = MILLI;

    fn limiter() -> WriteLimiter {
        // target 100 ms, budget 500 ms, 2 to 10 in flight.
        WriteLimiter::new(100, 500, 2, 10)
    }

    #[test]
    fn writes_up_to_the_limit_are_admitted_and_the_next_is_refused() {
        let l = WriteLimiter::new(100, 500, 1, 3);
        let held: Vec<_> = (0..3).map(|_| l.admit(0).expect("admitted")).collect();
        let refused = l.admit(0).expect_err("over the limit");
        assert_eq!(refused.retry_after_secs, 1);
        assert!(refused.reason.contains("3 writes already waiting, limit 3"));
        assert_eq!(l.stats().refused, 1);
        drop(held);
        assert_eq!(l.stats().in_flight, 0);
        assert!(l.admit(0).is_ok(), "a dropped permit frees its slot");
    }

    #[test]
    fn a_slow_smoothed_append_cuts_the_limit_by_a_third_and_two_cuts_in_500ms_are_one() {
        let l = limiter();
        l.on_append(0, 200 * MS, true);
        assert_eq!(l.stats().limit, 6, "10 * 2/3 = 6.67");
        l.on_append(400 * MS, 200 * MS, true);
        assert_eq!(l.stats().limit, 6, "inside 500 ms of the last cut");
        l.on_append(600 * MS, 200 * MS, true);
        assert_eq!(l.stats().limit, 4, "6 * 2/3");
    }

    #[test]
    fn a_failed_append_cuts_at_once_and_the_limit_never_passes_the_minimum() {
        let l = limiter();
        let mut now = 0;
        for _ in 0..10 {
            l.on_append(now, MS, false);
            now += SECOND;
        }
        assert_eq!(l.stats().limit, 2);
    }

    #[test]
    fn a_single_slow_sample_among_fast_ones_is_not_a_cut() {
        let l = limiter();
        l.on_append(0, 10 * MS, true);
        // 0.8 * 10 + 0.2 * 300 = 68 ms: under the target.
        l.on_append(SECOND, 300 * MS, true);
        assert_eq!(l.stats().limit, 10);
    }

    #[test]
    fn fast_appends_grow_the_limit_to_the_maximum_and_no_further() {
        let l = limiter();
        l.on_append(0, 200 * MS, true);
        assert_eq!(l.stats().limit, 6);
        // The smoothed time must fall under half the target (50 ms) first.
        let mut now = SECOND;
        for _ in 0..40 {
            l.on_append(now, MS, true);
            now += SECOND;
        }
        assert_eq!(l.stats().limit, 10);
    }

    #[test]
    fn appends_between_half_the_target_and_the_target_grow_it_by_the_additive_step() {
        let l = WriteLimiter::new(100, 500, 2, 100);
        // A failed append at 75 ms cuts 100 to 66.67 and smooths to 75 ms.
        l.on_append(0, 75 * MS, false);
        assert_eq!(l.stats().limit, 66);
        // Each 75 ms success adds 4 / limit, about 0.06 of a slot: thirty
        // of them add about 1.8, where the geometric step would reach 100.
        let mut now = SECOND;
        for _ in 0..30 {
            l.on_append(now, 75 * MS, true);
            now += SECOND;
        }
        assert_eq!(l.stats().limit, 68);
    }

    #[test]
    fn writes_beyond_the_minimum_are_refused_while_appends_exceed_the_budget() {
        let l = limiter();
        // A smoothed 3 s append: over the 500 ms budget.
        l.on_append(0, 3_000 * MS, true);
        let a = l.admit(SECOND).expect("the first is within the minimum");
        let b = l.admit(SECOND).expect("so is the second");
        let refused = l.admit(SECOND).expect_err("the third is beyond it");
        assert_eq!(refused.retry_after_secs, 4, "3 s of pressure, plus one");
        drop((a, b));
    }

    #[test]
    fn the_retry_hint_never_passes_thirty_seconds() {
        let l = limiter();
        l.on_append(0, 600_000 * MS, true);
        let _a = l.admit(SECOND).expect("within the minimum");
        let _b = l.admit(SECOND).expect("within the minimum");
        assert_eq!(l.admit(SECOND).expect_err("refused").retry_after_secs, 30);
    }

    #[test]
    fn a_write_waiting_past_the_target_cuts_the_limit_though_no_append_ended() {
        let l = limiter();
        let _stuck = l.admit(0).expect("admitted");
        assert_eq!(l.stats().limit, 10);
        let _next = l.admit(150 * MS).expect("the limit is cut, not yet met");
        assert_eq!(
            l.stats().limit,
            6,
            "the oldest waited 150 ms, over the 100 ms target"
        );
    }

    #[test]
    fn a_write_waiting_past_the_budget_refuses_new_ones_beyond_the_minimum() {
        let l = limiter();
        let _a = l.admit(0).expect("admitted");
        let _b = l.admit(0).expect("admitted");
        let refused = l.admit(2 * SECOND).expect_err("the oldest has waited 2 s");
        assert_eq!(refused.retry_after_secs, 3);
    }

    #[test]
    fn the_hint_a_refusal_would_give_is_asked_without_refusing() {
        let l = limiter();
        assert_eq!(l.retry_after(0), 1, "no pressure");
        let _a = l.admit(0).expect("admitted");
        assert_eq!(l.retry_after(2 * SECOND), 3, "the oldest has waited 2 s");
        assert_eq!(l.retry_after(500 * SECOND), 30, "never over 30");
    }

    #[test]
    fn the_first_sample_is_taken_whole_and_later_ones_smooth() {
        let l = limiter();
        l.on_append(0, 40 * MS, true);
        assert_eq!(l.stats().append_millis, 40);
        l.on_append(SECOND, 90 * MS, true);
        assert_eq!(l.stats().append_millis, 50, "0.8 * 40 + 0.2 * 90");
    }

    #[test]
    fn a_budget_below_the_target_is_raised_to_it_and_bounds_are_ordered() {
        let l = WriteLimiter::new(1_000, 10, 0, 0);
        assert_eq!(
            l.stats().limit,
            1,
            "the minimum is at least 1 and the maximum its floor"
        );
        let l = WriteLimiter::new(1_000, 10, 1, 10);
        // 900 ms is over the given budget but under the raised one (1 s):
        // a write beyond the minimum is admitted.
        l.on_append(0, 900 * MS, true);
        let _a = l.admit(SECOND).expect("within the minimum");
        let _b = l.admit(SECOND).expect("under the raised budget");
        // 1.5 s smoothed to 1.02 s is over it.
        l.on_append(SECOND, 1_500 * MS, true);
        assert!(l.admit(SECOND).is_err());
    }
}
