//! The slow start of a joining reader: how much of its share of shards a
//! search node takes, from its lease and the clock alone, so every node
//! agrees without asking. The node decides how warm it is and publishes it
//! (`ramp_permille`); time only bounds that, from below by a floor curve (a
//! node cannot claim to be warm sooner) and from above by a ceiling curve (a
//! stuck or idle node is fully ramped by `max_duration`).
//! Integer arithmetic only, so the Java shell computes the same numbers.

use oss_format::NodeLease;

use super::hash::{ShardKey, decimal, murmur3};

/// The full ramp: the node takes all its share.
pub const FULL: i64 = 1000;

/// The bounds on a ramp, milliseconds. `max_ms <= 0` is off: every node is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RampConfig {
    /// The soonest a node may be fully ramped.
    pub min_ms: i64,
    /// The latest a node is fully ramped, whatever it says.
    pub max_ms: i64,
}

impl RampConfig {
    /// No ramp: every reader takes its full share.
    pub const OFF: Self = Self {
        min_ms: 0,
        max_ms: 0,
    };

    /// Whether the ramp is on.
    pub fn is_on(&self) -> bool {
        self.max_ms > 0
    }
}

/// `(now - since) * 1000 / duration` clamped to 0 to 1000; 1000 for a
/// duration that is not positive.
fn curve(now: i64, since: i64, duration: i64) -> i64 {
    if duration <= 0 {
        return FULL;
    }
    let age = i128::from(now).saturating_sub(i128::from(since));
    let permille = age
        .saturating_mul(1000)
        .checked_div(i128::from(duration))
        .unwrap_or(i128::from(FULL));
    i64::try_from(permille.clamp(0, i128::from(FULL))).unwrap_or(FULL)
}

/// The share of its shards, 0 to 1000, that `lease`'s node takes at `now`.
pub fn ramp_permille(lease: &NodeLease, now: i64, config: RampConfig) -> i64 {
    let Some(since) = lease.search_since.filter(|_| config.is_on()) else {
        return FULL;
    };
    if since > now {
        return FULL;
    }
    let ceiling = curve(now, since, config.max_ms);
    match lease.ramp_permille() {
        None => ceiling,
        Some(said) => {
            let floor = curve(now, since, config.min_ms);
            ceiling.max(i64::from(said).min(floor))
        }
    }
}

/// A shard's fixed admission value for a node, 0 to 999: the node takes the
/// shard once its ramp is above it, and keeps it as the ramp rises.
pub fn admission(node_id: &str, key: ShardKey<'_>) -> i64 {
    let mut digits = [0u8; 11];
    let shard = decimal(key.shard, &mut digits);
    i64::from(murmur3(&[node_id, "/", key.index, "~", shard])).rem_euclid(1000)
}

/// Whether the node takes `key` at `ramp`.
pub fn admitted(node_id: &str, key: ShardKey<'_>, ramp: i64) -> bool {
    ramp >= FULL || admission(node_id, key) < ramp
}

/// What the joiner's controller reads: searches served and store GETs made
/// by this node so far (GETs in thousandths of a request, as the ledger counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    /// Searches this node has answered.
    pub searches: u64,
    /// Store GET-class requests it has made, in thousandths.
    pub gets_milli: u64,
}

/// How the joiner's controller steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerConfig {
    /// How far a qualifying window raises the ramp.
    pub step: i32,
    /// The most store GETs a search may average, in thousandths, for the window to qualify.
    pub warm_gets_per_search_milli: u64,
    /// The fewest searches a window needs to say anything.
    pub min_searches: u64,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            step: 100,
            warm_gets_per_search_milli: 2_000,
            min_searches: 20,
        }
    }
}

/// The joiner's own judgement of how warm it is: a window of searches that
/// each needed about no store read raises its ramp a step; a window with too
/// few searches, or a cold one, holds it. Time bounds the answer elsewhere
/// ([`ramp_permille`]), so a node with no traffic is still fully ramped.
#[derive(Debug, Clone)]
pub struct RampController {
    config: ControllerConfig,
    ramp: i32,
    seen: Totals,
    idle: u32,
}

/// Renewals with too few searches to judge, after which the ramp steps up
/// anyway: a node admitted to no shard it holds gets no search, so waiting
/// for evidence would idle it until the ceiling.
const IDLE_RENEWALS: u32 = 3;

impl RampController {
    /// A controller that counts from `start`, at the first step: a node
    /// ranked behind every shard gets no search, so it could never gather
    /// the evidence to move; the step admits it a tenth of its shards, and
    /// the floor curve still holds that back until the time has passed.
    pub fn new(config: ControllerConfig, start: Totals) -> Self {
        Self {
            config,
            ramp: config.step.clamp(0, 1000),
            seen: start,
            idle: 0,
        }
    }

    /// The ramp it publishes.
    pub fn ramp(&self) -> i32 {
        self.ramp
    }

    /// Reads the totals at a lease renewal and returns the ramp to publish.
    pub fn observe(&mut self, now: Totals) -> i32 {
        let searches = now.searches.saturating_sub(self.seen.searches);
        if searches < self.config.min_searches.max(1) {
            // Too few searches to judge: little traffic means little cold cost,
            // so after a few renewals the ramp steps up and admits more shards.
            self.idle = self.idle.saturating_add(1);
            if self.idle >= IDLE_RENEWALS {
                self.idle = 0;
                self.seen = now;
                self.ramp = self.ramp.saturating_add(self.config.step).clamp(0, 1000);
            }
            return self.ramp;
        }
        self.idle = 0;
        let gets = now.gets_milli.saturating_sub(self.seen.gets_milli);
        self.seen = now;
        let per_search = gets.checked_div(searches).unwrap_or(u64::MAX);
        if per_search <= self.config.warm_gets_per_search_milli {
            self.ramp = self
                .ramp
                .saturating_add(self.config.step)
                .clamp(0, i32::try_from(FULL).unwrap_or(1000));
        }
        self.ramp
    }
}

static SEARCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Counts one search this node answered, for the joiner's controller.
pub fn count_search() {
    SEARCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Searches this node has answered.
pub fn searches_total() -> u64 {
    SEARCHES.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: RampConfig = RampConfig {
        min_ms: 30_000,
        max_ms: 300_000,
    };

    fn lease(since: Option<i64>, said: Option<i32>) -> NodeLease {
        let mut l = NodeLease::new("n", "e", "a", 0);
        l.search_since = since;
        l.set_ramp_permille(said);
        l
    }

    #[test]
    fn off_or_unstamped_is_full() {
        assert_eq!(
            ramp_permille(&lease(Some(0), Some(0)), 1, RampConfig::OFF),
            FULL
        );
        assert_eq!(ramp_permille(&lease(None, Some(0)), 1, CFG), FULL);
        assert_eq!(ramp_permille(&lease(Some(10), None), 5, CFG), FULL);
    }

    #[test]
    fn silent_node_follows_the_ceiling() {
        let l = lease(Some(1_000), None);
        assert_eq!(ramp_permille(&l, 1_000, CFG), 0);
        assert_eq!(ramp_permille(&l, 151_000, CFG), 500);
        assert_eq!(ramp_permille(&l, 301_000, CFG), FULL);
        assert_eq!(ramp_permille(&l, i64::MAX, CFG), FULL);
    }

    #[test]
    fn the_node_cannot_beat_the_floor_or_fall_below_the_ceiling() {
        let eager = lease(Some(0), Some(1000));
        assert_eq!(ramp_permille(&eager, 15_000, CFG), 500);
        assert_eq!(ramp_permille(&eager, 30_000, CFG), FULL);
        let cold = lease(Some(0), Some(0));
        assert_eq!(ramp_permille(&cold, 150_000, CFG), 500);
        assert_eq!(ramp_permille(&cold, 300_000, CFG), FULL);
        let mid = lease(Some(0), Some(200));
        assert_eq!(ramp_permille(&mid, 60_000, CFG), 200);
    }

    #[test]
    fn extremes_do_not_overflow() {
        let l = lease(Some(i64::MIN), Some(1000));
        assert_eq!(ramp_permille(&l, i64::MAX, CFG), FULL);
        let l = lease(Some(0), None);
        assert_eq!(ramp_permille(&l, -5, CFG), FULL);
    }

    #[test]
    fn admission_is_fixed_and_spread() {
        let key = ShardKey::new("idx", 3);
        assert_eq!(admission("n1", key), admission("n1", key));
        let low = (0..2000)
            .filter(|s| admission("n1", ShardKey::new("idx", *s)) < 250)
            .count();
        assert!((400..600).contains(&low), "{low}");
        assert!(admitted("n1", key, FULL));
        assert!(!admitted("n1", key, 0));
    }

    fn totals(searches: u64, gets: u64) -> Totals {
        Totals {
            searches,
            gets_milli: gets.saturating_mul(1000),
        }
    }

    #[test]
    fn controller_steps_up_on_warm_windows_and_holds_otherwise() {
        let mut c = RampController::new(ControllerConfig::default(), totals(0, 0));
        assert_eq!(c.ramp(), 100, "the first step is the start");
        assert_eq!(
            c.observe(totals(19, 0)),
            100,
            "too few searches say nothing"
        );
        assert_eq!(c.observe(totals(20, 0)), 200);
        // 30 searches, 90 GETs: 3 per search, cold: hold.
        assert_eq!(c.observe(totals(50, 90)), 200);
        // The cold window was consumed; the next window is judged on its own.
        assert_eq!(c.observe(totals(70, 90 + 20)), 300);
        for i in 0..20u64 {
            c.observe(totals(90 + i * 20, 110));
        }
        assert_eq!(c.ramp(), 1000);
    }

    #[test]
    fn a_window_short_of_searches_keeps_collecting() {
        let mut c = RampController::new(ControllerConfig::default(), totals(0, 0));
        assert_eq!(c.observe(totals(10, 0)), 100);
        assert_eq!(c.observe(totals(20, 0)), 200, "the first ten still count");
    }

    #[test]
    fn a_node_that_gets_no_search_steps_up_after_a_few_renewals() {
        let mut c = RampController::new(ControllerConfig::default(), totals(0, 0));
        assert_eq!(c.observe(totals(0, 0)), 100);
        assert_eq!(c.observe(totals(1, 0)), 100);
        assert_eq!(c.observe(totals(2, 0)), 200, "the third idle renewal steps");
        assert_eq!(c.observe(totals(2, 0)), 200, "and counts again from there");
        // A judged window resets the count.
        assert_eq!(c.observe(totals(30, 0)), 300);
        assert_eq!(c.observe(totals(30, 0)), 300);
        assert_eq!(c.observe(totals(30, 0)), 300);
        assert_eq!(c.observe(totals(30, 0)), 400);
        for _ in 0..40 {
            c.observe(totals(30, 0));
        }
        assert_eq!(c.ramp(), 1000);
    }

    #[test]
    fn the_counter_counts() {
        let before = searches_total();
        count_search();
        assert!(searches_total() > before);
    }
}
