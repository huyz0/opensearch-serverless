//! Keeping a search in the searching node's zone. Ported from
//! `cluster/Zones.java` and `cluster/ZonePreference.java`: cross-zone
//! traffic is billed, so a shard searched at least `min_per_second` from
//! here is read by a reader in this zone when placement's first choice is
//! in another, unless that reader is busier than the first choice by more
//! than `spill_in_flight` searches. A cold shard keeps placement's answer,
//! so a node does not pin every shard it ever searched to one local reader.

use oss_format::NodeLease;

use super::hash::ShardKey;
use super::rendezvous::readers_for;
use super::spread::HotShardSpread;

/// The rate below which placement's choice is kept
/// (`serverless.search.zone_local.min_per_second`).
pub const DEFAULT_MIN_PER_SECOND: f64 = 1.0;
/// How many more searches a zone-local reader may run than placement's
/// choice before the search spills to it
/// (`serverless.search.zone_local.spill_in_flight`).
pub const DEFAULT_SPILL_IN_FLIGHT: usize = 8;

/// A zone as the Java compares it: blank is no zone.
pub fn normalise(zone: Option<&str>) -> Option<&str> {
    zone.map(str::trim).filter(|z| !z.is_empty())
}

/// Whether `a` and `b` name the same zone; no zone matches nothing.
pub fn same(a: Option<&str>, b: Option<&str>) -> bool {
    a.is_some() && a == b
}

/// Whether traffic from `a` to `b` crosses zones: both known and differing.
pub fn crosses(a: Option<&str>, b: Option<&str>) -> bool {
    a.is_some() && b.is_some() && a != b
}

/// The zone `node_id`'s lease names among `members`.
pub fn zone_of<'a>(members: &'a [NodeLease], node_id: &str) -> Option<&'a str> {
    members
        .iter()
        .find(|l| l.node_id == node_id)
        .and_then(NodeLease::zone)
}

/// The zone preference: when a search stays in this node's zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZonePreference {
    min_per_second: f64,
    spill_in_flight: usize,
}

impl Default for ZonePreference {
    fn default() -> Self {
        Self {
            min_per_second: DEFAULT_MIN_PER_SECOND,
            spill_in_flight: DEFAULT_SPILL_IN_FLIGHT,
        }
    }
}

impl ZonePreference {
    /// A preference that keeps shards searched at least `min_per_second`
    /// local; `None` for a negative or unordered rate.
    pub fn new(min_per_second: f64, spill_in_flight: usize) -> Option<Self> {
        (min_per_second >= 0.0).then_some(Self {
            min_per_second,
            spill_in_flight,
        })
    }

    /// Placement's `ordered` readers for `key`, with a reader in
    /// `local_zone` first when the shard is warm enough and that reader not
    /// too busy (`order`).
    pub fn order<'a>(
        &self,
        key: ShardKey<'_>,
        ordered: &[&'a str],
        members: &'a [NodeLease],
        role: &str,
        local_zone: Option<&str>,
        spread: &HotShardSpread,
    ) -> Vec<&'a str> {
        // ALLOC: a copy of the ordered readers, a handful.
        let keep = || ordered.to_vec();
        let Some(first) = ordered.first() else {
            return keep();
        };
        if local_zone.is_none() || spread.per_second(key) < self.min_per_second {
            return keep();
        }
        if same(local_zone, zone_of(members, first)) {
            return keep();
        }
        let in_zone = members.iter().filter(|l| same(local_zone, l.zone()));
        let ranked = readers_for(key, in_zone, role, spread.max_readers());
        let Some(chosen) = spread.order_counted(key, &ranked).first().copied() else {
            return keep();
        };
        if spread.in_flight(chosen) > spread.in_flight(first).saturating_add(self.spill_in_flight) {
            return keep();
        }
        // ALLOC: one more than the ordered readers, a handful.
        let mut out = Vec::with_capacity(ordered.len().saturating_add(1));
        out.push(chosen);
        out.extend(ordered.iter().copied().filter(|id| *id != chosen));
        out
    }
}
