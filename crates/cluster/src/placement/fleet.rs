//! Which writer takes a shard, and when a writer should hand shards off.
//! Ported from the pure decisions of `node/FleetPlacement.java`: each one is
//! a function of the leases this node holds, so every node answers the same
//! from the same view, and a disagreement between views is settled by the
//! shard head's compare-and-swap, not by these answers.

use std::collections::HashMap;
use std::sync::Mutex;

use oss_format::{NodeLease, java_order};

use super::hash::{ShardKey, decimal, java_hash_code};
use super::rendezvous::owner;
use super::zone::same;
use super::{ROLE_INGEST, has_role};
use crate::lease::lock;

/// The share of capacity a fleet is sized to use.
pub const TARGET_UTILISATION: f64 = 0.8;
/// The share of its cap above which a writer is nearly full.
pub const HAND_OFF_ABOVE: f64 = 0.9;
/// The most a member may hold, as a share of its cap, to take a hand-off.
pub const ROOM_AT_OR_BELOW: f64 = 0.75;
/// How far above the fleet's mean a writer holds before it hands off.
pub const HAND_OFF_ABOVE_MEAN: f64 = 1.5;
/// Where a hand-off above the mean stops.
pub const HAND_OFF_DOWN_TO_MEAN: f64 = 1.25;
/// The share of its cap a writer holds before the mean matters.
pub const MEAN_MATTERS_FROM: f64 = 0.25;
/// The most departed members whose zone is remembered.
const MAX_DEPARTED: usize = 4096;

/// What this node holds against what it may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Holding {
    /// Shards held now.
    pub held: i32,
    /// The most it holds; 0 or less for no bound.
    pub cap: i32,
}

/// How many writers the fleet's reported load wants (`FleetSize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FleetSize {
    /// Members reporting a load.
    pub members: i32,
    /// Writers wanted at the target utilisation, at least 1; -1 when nobody
    /// reports a load yet.
    pub wanted: i32,
    /// Shards in use or refused across them.
    pub demand: i64,
}

/// One node's view of the fleet at one moment: the leases it holds, the
/// time, and itself.
#[derive(Debug, Clone, Copy)]
pub struct FleetView<'a> {
    /// The members as last read (`Membership::current`).
    pub members: &'a [NodeLease],
    /// The time, in the leases' milliseconds.
    pub now: i64,
    /// The lease lifetime.
    pub ttl: i64,
    /// This node's id.
    pub local: &'a str,
    /// This node's own load.
    pub holding: Holding,
}

impl<'a> FleetView<'a> {
    /// Writers that are live by an unexpired lease, this node aside.
    fn live_writers(&self) -> impl Iterator<Item = &'a NodeLease> + '_ {
        self.members.iter().filter(move |l| {
            l.node_id != self.local && !l.is_expired_at(self.now) && has_role(l, ROLE_INGEST)
        })
    }

    /// How many writers the load the members report wants (`fleetSize`).
    /// A copy is counted up to a lifetime past its expiry, since this node
    /// re-reads a lease only once its copy runs out.
    pub fn fleet_size(&self) -> FleetSize {
        let (mut members, mut demand, mut caps) = (0i32, 0i64, 0i64);
        for l in self.members {
            if l.revoked
                || self.now >= l.expires_at_millis.saturating_add(self.ttl)
                || !has_role(l, ROLE_INGEST)
                || l.in_use < 0
                || l.cap <= 0
            {
                continue;
            }
            members = members.saturating_add(1);
            demand = demand
                .saturating_add(i64::from(l.in_use))
                .saturating_add(i64::from(l.refused.max(0)));
            caps = caps.saturating_add(i64::from(l.cap));
        }
        if members == 0 {
            return FleetSize {
                members: 0,
                wanted: -1,
                demand: 0,
            };
        }
        let per_node = caps as f64 / f64::from(members) * TARGET_UTILISATION;
        let wanted = (demand as f64 / per_node).ceil().max(1.0);
        FleetSize {
            members,
            // Saturates as the Java's `(int)` does.
            wanted: wanted as i32,
            demand,
        }
    }

    /// Whether this node holds at least `HAND_OFF_ABOVE` of its cap.
    pub fn nearly_full(&self) -> bool {
        let Holding { held, cap } = self.holding;
        cap > 0 && f64::from(held) >= f64::from(cap) * HAND_OFF_ABOVE
    }

    /// The live writer with the smallest share of its cap, at most
    /// `ROOM_AT_OR_BELOW`; the later of equals (`memberWithRoom`).
    pub fn member_with_room(&self) -> Option<&'a str> {
        let mut best = None;
        let mut best_share = ROOM_AT_OR_BELOW;
        for l in self.live_writers() {
            if l.held < 0 || l.cap <= 0 {
                continue;
            }
            let share = f64::from(l.held) / f64::from(l.cap);
            if share <= best_share {
                best = Some(l.node_id.as_str());
                best_share = share;
            }
        }
        best
    }

    /// The mean held across this node and the live writers reporting it, or
    /// -1 with nobody else (`fleetMeanHeld`).
    pub fn fleet_mean_held(&self) -> f64 {
        let mut sum = i64::from(self.holding.held);
        let mut members = 1i32;
        for l in self.live_writers().filter(|l| l.held >= 0) {
            sum = sum.saturating_add(i64::from(l.held));
            members = members.saturating_add(1);
        }
        if members < 2 {
            -1.0
        } else {
            sum as f64 / f64::from(members)
        }
    }

    /// Whether this node holds well above the fleet's mean, and enough of
    /// its cap for that to matter (`aboveFleetShare`).
    pub fn above_fleet_share(&self) -> bool {
        let Holding { held, cap } = self.holding;
        let mean = self.fleet_mean_held();
        cap > 0
            && mean > 0.0
            && f64::from(held) >= f64::from(cap) * MEAN_MATTERS_FROM
            && f64::from(held) > mean * HAND_OFF_ABOVE_MEAN
    }

    /// The live writer holding fewest, below the mean; the first of equals
    /// (`memberBelowMean`).
    pub fn member_below_mean(&self) -> Option<&'a str> {
        let mean = self.fleet_mean_held();
        if mean <= 0.0 {
            return None;
        }
        let mut best: Option<&NodeLease> = None;
        for l in self.live_writers() {
            if l.held < 0 || f64::from(l.held) >= mean {
                continue;
            }
            if best.is_none_or(|b| l.held < b.held) {
                best = Some(l);
            }
        }
        best.map(|l| l.node_id.as_str())
    }

    /// Who this node should hand a shard to, if anyone (`handOffTarget`):
    /// nearly full, a member with room; well above the mean, the least
    /// loaded member below it.
    pub fn hand_off_target(&self) -> Option<&'a str> {
        if self.nearly_full()
            && let Some(roomy) = self.member_with_room()
        {
            return Some(roomy);
        }
        if self.above_fleet_share() {
            self.member_below_mean()
        } else {
            None
        }
    }

    /// The writer, other than this node, that `key`'s writes go to
    /// (`writerFor`): rendezvous over the live writers with room, or over
    /// every live writer when none has room.
    pub fn writer_for(&self, key: ShardKey<'_>) -> Option<&'a str> {
        let live = || self.live_writers().filter(|l| !l.revoked);
        let has_room = |l: &&NodeLease| l.cap <= 0 || l.held < 0 || l.held < l.cap;
        let roomy = owner(key, live().filter(has_room).map(|l| l.node_id.as_str()));
        roomy.or_else(|| owner(key, live().map(|l| l.node_id.as_str())))
    }

    /// The live writer that takes `key` from the dead `dead_owner`
    /// (`takeoverWinner`): of the members in the dead owner's zone, sorted by
    /// id, the first with room from a start the shard's hash picks; else of
    /// every live writer the same way; else that start regardless of room.
    /// `departed_zone` answers the zone of an owner whose lease is gone.
    pub fn takeover_winner(
        &self,
        key: ShardKey<'_>,
        dead_owner: &str,
        overcommit: f64,
        departed_zone: impl FnOnce() -> Option<String>,
    ) -> Option<&'a str> {
        // ALLOC: one entry a live writer, at most the fleet.
        let mut live: Vec<&NodeLease> = self
            .members
            .iter()
            .filter(|l| {
                l.node_id != dead_owner && !l.is_expired_at(self.now) && has_role(l, ROLE_INGEST)
            })
            // ALLOC: one entry a live writer, at most the fleet.
            .collect();
        if live.is_empty() {
            return None;
        }
        live.sort_by(|a, b| java_order(&a.node_id, &b.node_id));
        let known = self
            .members
            .iter()
            .find(|l| l.node_id == dead_owner)
            .and_then(NodeLease::zone);
        let departed;
        let dead_zone = match known {
            Some(z) => Some(z),
            None => {
                departed = departed_zone();
                departed.as_deref()
            }
        };
        let mut digits = [0u8; 11];
        let hash = java_hash_code(&[key.index, "#", decimal(key.shard, &mut digits)]);
        let start = |n: usize| {
            let n = i64::try_from(n).unwrap_or(i64::MAX);
            usize::try_from(i64::from(hash).rem_euclid(n)).unwrap_or(0)
        };
        // ALLOC: the live writers in the dead owner's zone, at most the fleet.
        let in_zone: Vec<&NodeLease> = live
            .iter()
            .copied()
            .filter(|l| same(dead_zone, l.zone()))
            // ALLOC: the live writers in the dead owner's zone, at most the fleet.
            .collect();
        for group in [&in_zone, &live] {
            if group.is_empty() {
                continue;
            }
            let first = start(group.len());
            let ring = group.iter().cycle().skip(first).take(group.len());
            if let Some(l) = ring
                .copied()
                .find(|l| self.has_takeover_room(l, overcommit))
            {
                return Some(l.node_id.as_str());
            }
        }
        live.get(start(live.len())).map(|l| l.node_id.as_str())
    }

    fn has_takeover_room(&self, l: &NodeLease, overcommit: f64) -> bool {
        let (held, cap) = if l.node_id == self.local {
            if self.holding.cap <= 0 {
                return true;
            }
            (self.holding.held, self.holding.cap)
        } else {
            if l.held < 0 || l.cap <= 0 {
                return true;
            }
            (l.held, l.cap)
        };
        f64::from(held) < f64::from(cap) + (f64::from(cap) * overcommit).floor()
    }
}

/// The zones of members whose lease is gone, remembered so a takeover of
/// each of their shards does not read the lease again (`departedZones`).
#[derive(Debug, Default)]
pub struct DepartedZones {
    // ALLOC: one entry a departed member, at most MAX_DEPARTED.
    zones: Mutex<HashMap<String, Option<String>>>,
}

impl DepartedZones {
    /// The zone `node_id` ran in, from memory or else from `read`; a failed
    /// read answers no zone and is asked again next time.
    pub fn zone_of<E>(
        &self,
        node_id: &str,
        read: impl FnOnce() -> Result<Option<NodeLease>, E>,
    ) -> Option<String> {
        if let Some(known) = lock(&self.zones).get(node_id) {
            return known.clone();
        }
        let zone = read().ok()?.and_then(|l| l.zone().map(str::to_owned));
        let mut zones = lock(&self.zones);
        if zones.len() >= MAX_DEPARTED {
            zones.clear();
        }
        // ALLOC: one entry a departed member, at most MAX_DEPARTED.
        zones.insert(node_id.to_owned(), zone.clone());
        zone
    }
}
