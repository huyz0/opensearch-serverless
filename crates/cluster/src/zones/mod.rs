//! Which zone a node runs in, whether two nodes share one, and the bytes a
//! node exchanges with peers in other zones. Ported from the Java shell's
//! `cluster/Zones`, `cluster/ZonePreference` and `transport/ZoneTraffic`.
//!
//! The zone is stock OpenSearch's awareness attribute: `node.attr.zone`, or
//! `node.attr.<name>` when `cluster.routing.allocation.awareness.attributes`
//! names another attribute first ([`attribute`], [`local_zone`]). A node
//! advertises it in its lease, so every peer learns it from the lease digest
//! it already reads and no request is added. The clouds bill traffic
//! between zones by the byte, so a peer choice with a same-zone option takes
//! it ([`ZonePreference`]), and [`ZoneTraffic`] counts what still crosses.
//!
//! The zone facts here sit below placement; only [`ZonePreference`], which
//! reorders placement's readers, uses placement's ranking and spread, and
//! placement re-exports it for its callers.
//!
//! Unknown is not a zone: a node with no attribute set is in no zone, and
//! two such nodes are not in the same one, so a fleet that never configured
//! zones neither looks like one zone nor reads as measured.

mod preference;
mod traffic;

use std::fmt::Write as _;

use oss_format::NodeLease;
use oss_types::settings::Source;

pub use preference::{DEFAULT_MIN_PER_SECOND, DEFAULT_SPILL_IN_FLIGHT, ZonePreference};
pub use traffic::{MAX_ACTIONS, OTHER_ACTION, Path, ZoneTraffic, ZoneTrafficStats};

/// The stock awareness setting; its first attribute names the zone
/// (`Zones.AWARENESS_ATTRIBUTES`).
pub const AWARENESS_ATTRIBUTES: &str = "cluster.routing.allocation.awareness.attributes";

/// The attribute stock OpenSearch deployments use for availability zones,
/// read when no awareness attribute is set (`Zones.DEFAULT_ATTRIBUTE`).
pub const DEFAULT_ATTRIBUTE: &str = "zone";

/// The prefix of a node attribute setting.
pub const NODE_ATTR_PREFIX: &str = "node.attr.";

/// The most entries of the legacy array form (`key.0`, `key.1`, ...) read
/// when looking for the first named awareness attribute.
pub const MAX_LIST_ENTRIES: usize = 64;

/// The node attribute that holds the zone: the first non-blank awareness
/// attribute, trimmed, else [`DEFAULT_ATTRIBUTE`] (`Zones.attribute`).
///
/// The list is read as the Java's `Settings.getAsList` reads flat settings:
/// a comma-separated value under the key, or the array form `key.0`,
/// `key.1`, ... when the key itself is not set.
pub fn attribute(settings: &dyn Source) -> &str {
    if let Some(list) = settings.value(AWARENESS_ATTRIBUTES) {
        return list
            .split(',')
            .map(str::trim)
            .find(|name| !name.is_empty())
            .unwrap_or(DEFAULT_ATTRIBUTE);
    }
    // ALLOC: one key buffer, reused for each entry; read once at startup.
    let mut key = String::with_capacity(AWARENESS_ATTRIBUTES.len().saturating_add(4));
    for i in 0..MAX_LIST_ENTRIES {
        key.clear();
        // A write to a String cannot fail.
        let _ = write!(key, "{AWARENESS_ATTRIBUTES}.{i}");
        let Some(name) = settings.value(&key) else {
            break;
        };
        if let Some(name) = normalise(Some(name)) {
            return name;
        }
    }
    DEFAULT_ATTRIBUTE
}

/// This node's zone, from `node.attr.<attribute>`; `None` when the node
/// declares none or a blank one (`Zones.localZone`).
pub fn local_zone(settings: &dyn Source) -> Option<&str> {
    let attribute = attribute(settings);
    // ALLOC: one key, read once at startup.
    let key = format!("{NODE_ATTR_PREFIX}{attribute}");
    normalise(settings.value(&key))
}

/// A zone as the Java compares it: trimmed, and blank is no zone.
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

/// The zone `node_id`'s lease names among `members`; `None` for a member
/// that is unknown or advertises none (`Zones.zoneOf`).
pub fn zone_of<'a>(members: &'a [NodeLease], node_id: &str) -> Option<&'a str> {
    members
        .iter()
        .find(|l| l.node_id == node_id)
        .and_then(NodeLease::zone)
}
