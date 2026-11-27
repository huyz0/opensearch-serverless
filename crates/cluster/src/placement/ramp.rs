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
}
