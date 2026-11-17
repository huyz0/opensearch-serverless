//! Bytes this node exchanged with its peers, split by whether they stayed in
//! its zone (`transport/ZoneTraffic`).
//!
//! Traffic between zones is what the clouds bill, so it is what every zone
//! preference is judged by. It is counted where a node talks to peers:
//! every forwarded request it sends and every answer it reads back, each
//! exchange once, by the node that started it, so a fleet's total is the sum
//! of its nodes' counts. Payload bytes, not wire bytes: framing, headers and
//! compression are left out, which keeps the count comparable across
//! settings and within a few percent of the bill.
//!
//! An exchange where either side declares no zone is counted apart as
//! [`Path::Unknown`] rather than guessed, so a fleet that never configured
//! zones reads as unmeasured, not as one zone.
//!
//! Each count takes one short lock; the Java's lock-free adders can replace
//! it if the forward path shows contention once it records here.
//!
//! Unlike the Java, the action names kept per path are bounded
//! ([`MAX_ACTIONS`]); past the bound an action's bytes are counted under
//! [`OTHER_ACTION`], so the totals stay exact and memory stays fixed.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use oss_format::NodeLease;

use super::{same, zone_of};

/// The most distinct action names counted per path; a node registers a few
/// dozen transport actions, so the bound is never reached in practice.
pub const MAX_ACTIONS: usize = 64;

/// The name bytes are counted under once a path holds [`MAX_ACTIONS`]
/// actions.
pub const OTHER_ACTION: &str = "_other";

/// Where an exchange went, relative to this node's zone (`ZoneTraffic.Path`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Path {
    /// Both sides in the same known zone.
    SameZone,
    /// Both zones known and different: the billed case.
    CrossZone,
    /// Either side declares no zone.
    Unknown,
}

impl Path {
    /// Every path, in the Java's order.
    pub const ALL: [Path; 3] = [Path::SameZone, Path::CrossZone, Path::Unknown];

    /// Classifies an exchange between `local` and `peer` (`pathBetween`).
    pub fn between(local: Option<&str>, peer: Option<&str>) -> Path {
        match (local, peer) {
            (Some(_), Some(_)) if same(local, peer) => Path::SameZone,
            (Some(_), Some(_)) => Path::CrossZone,
            _ => Path::Unknown,
        }
    }

    /// Classifies an exchange with `peer_id`, from the zone its lease among
    /// `members` advertises (`ShardRouter.pathTo`).
    pub fn to_peer(local: Option<&str>, members: &[NodeLease], peer_id: &str) -> Path {
        Path::between(local, zone_of(members, peer_id))
    }

    fn slot(self) -> usize {
        match self {
            Path::SameZone => 0,
            Path::CrossZone => 1,
            Path::Unknown => 2,
        }
    }
}

/// The counts at one moment, as the stats endpoint reports them under
/// `zone_traffic`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZoneTrafficStats {
    /// `same_zone_bytes`.
    pub same_zone_bytes: u64,
    /// `cross_zone_bytes`.
    pub cross_zone_bytes: u64,
    /// `unknown_zone_bytes`.
    pub unknown_zone_bytes: u64,
    /// `cross_zone_by_action`: only cross-zone bytes are broken down, since
    /// those are the ones to chase; sorted by action.
    pub cross_zone_by_action: BTreeMap<String, u64>,
}

/// The meter: bytes per path and action, safe to share between threads.
#[derive(Debug, Default)]
pub struct ZoneTraffic {
    by_action: Mutex<[BTreeMap<String, u64>; 3]>,
}

impl ZoneTraffic {
    /// An empty meter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts `bytes` sent or read for `action` on `path`; zero counts
    /// nothing (`record`).
    pub fn record(&self, path: Path, action: &str, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let mut counts = self.lock();
        let Some(map) = counts.get_mut(path.slot()) else {
            return;
        };
        if let Some(count) = map.get_mut(action) {
            *count = count.saturating_add(bytes);
            return;
        }
        let key = if map.len() < MAX_ACTIONS {
            action
        } else {
            OTHER_ACTION
        };
        // ALLOC: one action name the first time it is seen, at most
        // MAX_ACTIONS + 1 per path.
        let count = map.entry(key.to_owned()).or_insert(0);
        *count = count.saturating_add(bytes);
    }

    /// The bytes counted on `path`, all actions together (`total`).
    pub fn total(&self, path: Path) -> u64 {
        self.lock().get(path.slot()).map_or(0, sum)
    }

    /// The bytes counted on `path`, per action, sorted by action (`byAction`).
    pub fn by_action(&self, path: Path) -> BTreeMap<String, u64> {
        // ALLOC: a copy of at most MAX_ACTIONS + 1 counts, for stats.
        self.lock().get(path.slot()).cloned().unwrap_or_default()
    }

    /// The counts now, for the stats endpoint, read under one lock so the
    /// cross-zone total equals the sum of its breakdown.
    pub fn stats(&self) -> ZoneTrafficStats {
        let counts = self.lock();
        let total = |path: Path| counts.get(path.slot()).map_or(0, sum);
        ZoneTrafficStats {
            same_zone_bytes: total(Path::SameZone),
            cross_zone_bytes: total(Path::CrossZone),
            unknown_zone_bytes: total(Path::Unknown),
            // ALLOC: a copy of at most MAX_ACTIONS + 1 counts, for stats.
            cross_zone_by_action: counts
                .get(Path::CrossZone.slot())
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// Forgets every count, for a meter that measures one stretch of work
    /// (`reset`).
    pub fn reset(&self) {
        for map in self.lock().iter_mut() {
            map.clear();
        }
    }

    /// The counts, recovered from a poisoned lock: a panic elsewhere cannot
    /// leave them half-updated, since each update is one assignment.
    fn lock(&self) -> MutexGuard<'_, [BTreeMap<String, u64>; 3]> {
        self.by_action
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The sum of one path's counts.
fn sum(counts: &BTreeMap<String, u64>) -> u64 {
    counts.values().fold(0u64, |a, b| a.saturating_add(*b))
}
