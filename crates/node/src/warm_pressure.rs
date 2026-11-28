//! How fast a peer may serve joiners right now, from how its own searches
//! are doing: it backs off while their slowest ones are slower than they
//! were when no copy ran, and climbs back a step a second otherwise.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long one window of searches is.
const WINDOW: Duration = Duration::from_secs(1);

/// The fewest searches a window needs for its slowest to say anything.
const MIN_SEARCHES: u32 = 5;

/// A window's slowest search counts as slowed when it is this many percent
/// above the quiet baseline.
const SLOWED_PERCENT: u32 = 120;

/// The share of each new quiet window in the baseline, in percent.
const BASELINE_WEIGHT: f64 = 0.2;

/// The share of each new serving window in the recent figure.
const RECENT_WEIGHT: f64 = 0.3;

/// The slowest rate is the ceiling over this.
const FLOOR_DIVISOR: u64 = 8;

#[derive(Debug)]
struct State {
    window_start: Instant,
    slowest: Duration,
    searches: u32,
    served: bool,
    /// Seconds: the slowest search of a window with no copy, smoothed.
    baseline: Option<f64>,
    /// Seconds: the slowest search of a window with a copy, smoothed.
    recent: Option<f64>,
    rate: u64,
}

/// The serve rate a peer's search latency allows, between a floor and a
/// ceiling.
#[derive(Debug)]
pub struct Pressure {
    ceiling: u64,
    state: Mutex<State>,
}

impl Pressure {
    /// A controller allowing up to `ceiling` bytes a second, starting at a
    /// quarter of it; `now` opens the first window.
    #[must_use]
    pub fn new(ceiling: u64, now: Instant) -> Self {
        let ceiling = ceiling.max(1);
        Self {
            ceiling,
            state: Mutex::new(State {
                window_start: now,
                slowest: Duration::ZERO,
                searches: 0,
                served: false,
                baseline: None,
                recent: None,
                rate: (ceiling / 4).max(1),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Notes a search that took `took`, finished at `now`.
    pub fn record(&self, now: Instant, took: Duration) {
        let mut st = self.lock();
        self.roll(&mut st, now);
        st.slowest = st.slowest.max(took);
        st.searches = st.searches.saturating_add(1);
    }

    /// The rate for a batch of `bytes` asked for at `now`; the batch counts
    /// as the window's copy traffic.
    pub fn rate_for(&self, now: Instant, bytes: u64) -> u64 {
        let mut st = self.lock();
        self.roll(&mut st, now);
        if bytes > 0 {
            st.served = true;
        }
        st.rate
    }

    fn roll(&self, st: &mut State, now: Instant) {
        if now.saturating_duration_since(st.window_start) < WINDOW {
            return;
        }
        if st.searches >= MIN_SEARCHES {
            let slowest = st.slowest.as_secs_f64();
            if st.served {
                st.recent = Some(smooth(st.recent, slowest, RECENT_WEIGHT));
            } else {
                st.baseline = Some(smooth(st.baseline, slowest, BASELINE_WEIGHT));
            }
        }
        if st.served {
            let slowed = match (st.baseline, st.recent) {
                (Some(base), Some(recent)) => recent * 100.0 > base * f64::from(SLOWED_PERCENT),
                _ => false,
            };
            st.rate = if slowed {
                (st.rate / 2).max((self.ceiling / FLOOR_DIVISOR).max(1))
            } else {
                st.rate
                    .saturating_add(self.ceiling / FLOOR_DIVISOR)
                    .min(self.ceiling)
            };
        }
        st.window_start = now;
        st.slowest = Duration::ZERO;
        st.searches = 0;
        st.served = false;
    }
}

fn smooth(old: Option<f64>, new: f64, weight: f64) -> f64 {
    old.map_or(new, |o| o + (new - o) * weight)
}

#[cfg(test)]
mod tests;
