//! Placement: which node owns a shard's writes, which readers serve its
//! searches, and when a writer hands shards off, each computed by every
//! node from the leases alone so they agree without asking each other.
//!
//! Ported from the Java shell's `cluster/Rendezvous`, `ReaderPlacement`,
//! `HotShardSpread`, `ShardAssignment` and the
//! pure decisions of `node/FleetPlacement`, with the Java's hashes
//! ([`murmur3`], [`java_hash_code`]) so Rust and Java nodes pick the same
//! owner. Nothing here reads or writes the store: an answer is advice, and
//! the shard head's compare-and-swap is what makes one node the owner.

mod fleet;
mod hash;
mod rendezvous;
mod spread;

use oss_format::NodeLease;

pub use fleet::{
    DepartedZones, FleetSize, FleetView, HAND_OFF_ABOVE, HAND_OFF_ABOVE_MEAN,
    HAND_OFF_DOWN_TO_MEAN, Holding, MEAN_MATTERS_FROM, ROOM_AT_OR_BELOW, TARGET_UTILISATION,
};
pub use hash::{ShardKey, java_hash_code, murmur3};
pub use rendezvous::{owner, readers_for};
pub use spread::{
    DEFAULT_MAX_READERS, DEFAULT_TARGET_PER_SECOND, HotShardSpread, MAX_TRACKED, Nanos, Running,
    SpreadConfig,
};
// The zone preference lives in `zones`; re-exported here, where placement's
// callers already look for it.
pub use crate::zones::{
    DEFAULT_MIN_PER_SECOND, DEFAULT_SPILL_IN_FLIGHT, ZonePreference, crosses, normalise, same,
    zone_of,
};

/// The role of a node that owns shards and takes writes (`ROLE_INGEST`).
pub const ROLE_INGEST: &str = "ingest";
/// The role of a node that serves searches (`ROLE_SEARCH`).
pub const ROLE_SEARCH: &str = "search";

/// Whether `lease` names `role`.
pub fn has_role(lease: &NodeLease, role: &str) -> bool {
    lease.roles().iter().any(|r| r == role)
}

/// A shard this node owns, and the shard-head term it took it at
/// (`ShardAssignment`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ShardAssignment {
    index: String,
    shard: i32,
    term: u64,
}

impl ShardAssignment {
    /// `shard` of `index`, owned at `term`; `None` for term 0, at which the
    /// data plane refuses to activate a primary.
    pub fn new(index: impl Into<String>, shard: i32, term: u64) -> Option<Self> {
        (term >= 1).then(|| Self {
            index: index.into(),
            shard,
            term,
        })
    }

    /// The index.
    pub fn index(&self) -> &str {
        &self.index
    }

    /// The shard number.
    pub fn shard(&self) -> i32 {
        self.shard
    }

    /// The term ownership was taken at.
    pub fn term(&self) -> u64 {
        self.term
    }
}
