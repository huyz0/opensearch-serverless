//! The circuit breaker every plugin call goes through
//! ([ADR 0080](../../../docs/adr/0080-one-plugin-system-three-tiers.md)).
//!
//! Pure logic over an injected clock in milliseconds: closed until a run of
//! failures, or half of the recent calls, fail; then open (every call is
//! refused without being made) for a jittered time; then half-open, one
//! probe at a time. A failed probe reopens it for twice as long, up to a cap.

/// What trips and what resets the breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerConfig {
    /// Consecutive counted failures that open it.
    pub consecutive: u32,
    /// How many recent calls the failure share is read over (at most 32).
    pub window: u32,
    /// Calls the window needs before its share can open the breaker.
    pub min_calls: u32,
    /// How long the first opening lasts, in milliseconds.
    pub open_ms: u64,
    /// The longest an opening lasts after doublings, in milliseconds.
    pub max_open_ms: u64,
    /// The jitter on an opening, in thousandths either way.
    pub jitter_permille: u32,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            consecutive: 5,
            window: 20,
            min_calls: 10,
            open_ms: 30_000,
            max_open_ms: 300_000,
            jitter_permille: 200,
        }
    }
}

/// What the breaker says about one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Make the call; the breaker is closed.
    Call,
    /// Make the call as the single probe of a half-open breaker.
    Probe,
    /// Do not make the call.
    Reject,
}

/// How a call ended, as the breaker counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It worked.
    Success,
    /// It failed in a way that counts against the plugin.
    Failure,
    /// It ended in a way that says nothing (the caller's own error, capacity).
    Neutral,
}

/// Where the breaker is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Calls go through.
    Closed,
    /// Calls are refused until the time passes.
    Open,
    /// One probe may be in flight.
    HalfOpen,
}

/// One breaker.
#[derive(Debug, Clone)]
pub struct Breaker {
    config: BreakerConfig,
    state: BreakerState,
    run: u32,
    bits: u32,
    seen: u32,
    open_ms: u64,
    reopen_at: u64,
    probing: bool,
    rng: u64,
}

impl Breaker {
    /// A closed breaker; `seed` makes its jitter its own.
    pub fn new(config: BreakerConfig, seed: u64) -> Self {
        let config = BreakerConfig {
            window: config.window.clamp(1, 32),
            ..config
        };
        Self {
            open_ms: config.open_ms,
            config,
            state: BreakerState::Closed,
            run: 0,
            bits: 0,
            seen: 0,
            reopen_at: 0,
            probing: false,
            rng: seed | 1,
        }
    }

    /// Where it is now.
    pub fn state(&self) -> BreakerState {
        self.state
    }

    /// Whether a call arriving at `now_ms` may be made.
    pub fn allow(&mut self, now_ms: u64) -> Decision {
        match self.state {
            BreakerState::Closed => Decision::Call,
            BreakerState::Open if now_ms >= self.reopen_at => {
                self.state = BreakerState::HalfOpen;
                self.probing = true;
                Decision::Probe
            }
            BreakerState::Open => Decision::Reject,
            BreakerState::HalfOpen if self.probing => Decision::Reject,
            BreakerState::HalfOpen => {
                self.probing = true;
                Decision::Probe
            }
        }
    }

    /// Records how the call `decision` allowed ended at `now_ms`.
    pub fn record(&mut self, decision: Decision, outcome: Outcome, now_ms: u64) {
        match decision {
            Decision::Reject => {}
            Decision::Probe => self.probe_ended(outcome, now_ms),
            Decision::Call => {
                // A call that was admitted before the breaker opened says
                // nothing about the plugin now.
                if self.state == BreakerState::Closed {
                    self.counted(outcome, now_ms);
                }
            }
        }
    }

    fn probe_ended(&mut self, outcome: Outcome, now_ms: u64) {
        if self.state != BreakerState::HalfOpen {
            return;
        }
        self.probing = false;
        match outcome {
            Outcome::Success => {
                self.state = BreakerState::Closed;
                self.run = 0;
                self.bits = 0;
                self.seen = 0;
                self.open_ms = self.config.open_ms;
            }
            Outcome::Failure => {
                self.open_ms = self.open_ms.saturating_mul(2).min(self.config.max_open_ms);
                self.open(now_ms);
            }
            Outcome::Neutral => {}
        }
    }

    fn counted(&mut self, outcome: Outcome, now_ms: u64) {
        let failed = match outcome {
            Outcome::Neutral => return,
            Outcome::Success => {
                self.run = 0;
                0
            }
            Outcome::Failure => {
                self.run = self.run.saturating_add(1);
                1
            }
        };
        let mask = u32::MAX
            .checked_shr(32_u32.saturating_sub(self.config.window))
            .unwrap_or(u32::MAX);
        self.bits = (self.bits.rotate_left(1) & !1 & mask) | failed;
        self.seen = self.seen.saturating_add(1).min(self.config.window);
        let failures = self.bits.count_ones();
        let share = self.seen >= self.config.min_calls && failures.saturating_mul(2) >= self.seen;
        if self.run >= self.config.consecutive || share {
            self.open(now_ms);
        }
    }

    fn open(&mut self, now_ms: u64) {
        self.state = BreakerState::Open;
        self.probing = false;
        self.reopen_at = now_ms.saturating_add(self.jittered(self.open_ms));
    }

    fn jittered(&mut self, ms: u64) -> u64 {
        // xorshift64: a seed's own sequence, no clock and no global state.
        self.rng ^= self.rng.wrapping_shl(13);
        self.rng ^= self.rng.wrapping_shr(7);
        self.rng ^= self.rng.wrapping_shl(17);
        let span = u64::from(self.config.jitter_permille)
            .saturating_mul(2)
            .saturating_add(1);
        let step = self.rng.checked_rem(span).unwrap_or(0);
        let permille = 1000_u64
            .saturating_add(step)
            .saturating_sub(u64::from(self.config.jitter_permille));
        ms.saturating_mul(permille).checked_div(1000).unwrap_or(ms)
    }
}
