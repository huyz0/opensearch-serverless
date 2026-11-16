//! Node leases: one register a node under `cluster/members/`, the members
//! index beside them, and the lease digest.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/membership/`
//! (`BlobLeaseMembership`, `MembershipDelta`, `MembershipSource`,
//! `LeaseRevokedException`) and `metadata/LeaseLivenessOracle.java`.
//!
//! **What the expiry check assumes.** A lease carries an expiry its writer
//! stamped, compared against the reader's clock. A taker treats a lease as
//! live until that stamp, exactly as written; the holder treats its own as
//! gone a skew margin (`ttl / 32`, about a second at the default 30 s)
//! before it ([`Membership::self_valid_at`]). The margin is for liveness
//! only: it makes a holder stop before a taker starts, so the two rarely
//! overlap. Correctness never rests on clocks: a taker swaps the lease to a
//! revoked copy before it takes a shard, the holder's next renewal fails on
//! the change ([`ClusterError::Revoked`](crate::ClusterError::Revoked)), and
//! the writes themselves are fenced by the conditional creates and swaps on
//! the write log and shard heads, which a lapsed holder loses.
//!
//! **No LIST.** The Java lists `lease-*` once when the members index is
//! missing, for a deployment from before the index. This never does: a
//! missing index is created naming the enrolling node, and every Java node
//! at the ported revision writes the index when it joins.

mod membership;
mod oracle;
mod peers;
mod refresh;

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use oss_format::NodeLease;

pub use membership::Membership;
pub use oracle::LivenessOracle;
pub use refresh::Refreshed;

/// The Java's default lease lifetime (`NodeSettings.DEFAULT_LEASE_TTL_MILLIS`).
pub const DEFAULT_TTL_MILLIS: i64 = 30_000;

/// The holder's skew margin as a fraction of the lifetime
/// (`SKEW_MARGIN_DIVISOR`).
pub const SKEW_MARGIN_DIVISOR: i64 = 32;

/// How many lifetimes a lease may be expired before a refresh unlists it
/// (`DEAD_LEASE_PRUNE_TTLS`).
pub const DEAD_LEASE_PRUNE_TTLS: i64 = 10;

/// How many times a revocation reads and swaps before answering no
/// (`REVOKE_ATTEMPTS`).
pub const REVOKE_ATTEMPTS: u32 = 8;

/// How many times an enrolment reads and swaps the index on one renewal; a
/// node that loses them all tries again on its next renewal.
pub const ENROL_ATTEMPTS: u32 = 3;

/// The default bound on members, which sizes the index and digest buffers:
/// a fleet is hundreds of nodes, and beyond that another fleet.
pub const DEFAULT_MAX_MEMBERS: usize = 512;

/// How long a young snapshot answers without reading whether the members
/// index moved (`GENERATION_PROBE_INTERVAL_MILLIS`).
pub const GENERATION_PROBE_INTERVAL: Duration = Duration::from_secs(1);

/// The wall clock leases are stamped and judged by, and how a contended
/// enrolment pauses.
pub trait LeaseClock: Send + Sync {
    /// Milliseconds since the epoch.
    fn now_millis(&self) -> i64;
    /// Blocks for `pause`.
    fn pause(&self, pause: Duration);
}

/// The system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl LeaseClock for SystemClock {
    fn now_millis(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }

    fn pause(&self, pause: Duration) {
        std::thread::sleep(pause);
    }
}

/// How a node's membership is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseConfig {
    /// How far ahead a renewal stamps its expiry; positive.
    pub ttl_millis: i64,
    /// The most members the index and digest buffers hold.
    pub max_members: usize,
    /// Whether the store's conditional delete was verified, which the node
    /// supplies from its own probe of the store: only then are released and pruned
    /// leases deleted rather than left unlisted.
    pub conditional_deletes: bool,
}

impl Default for LeaseConfig {
    fn default() -> Self {
        Self {
            ttl_millis: DEFAULT_TTL_MILLIS,
            max_members: DEFAULT_MAX_MEMBERS,
            conditional_deletes: false,
        }
    }
}

/// Who joined and who left between two refreshes (`MembershipDelta`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MembershipDelta {
    /// Members live now that were not before.
    pub joined: Vec<NodeLease>,
    /// Members live before that are not now.
    pub left: Vec<NodeLease>,
}

impl MembershipDelta {
    /// Whether nobody joined or left.
    pub fn is_empty(&self) -> bool {
        self.joined.is_empty() && self.left.is_empty()
    }
}

/// One member as a delta compares it: everything a lease says but when it
/// runs out, so a renewal is neither a join nor a departure.
fn same_member(a: &NodeLease, b: &NodeLease) -> bool {
    a.node_id == b.node_id
        && a.ephemeral_id == b.ephemeral_id
        && a.address == b.address
        && a.roles() == b.roles()
        && a.revoked == b.revoked
}

/// Locks `mutex`, taking it over if a thread panicked holding it: every
/// change under these locks is one whole step.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `lease` renewed until `expires_at_millis`: unrevoked, everything else
/// kept (`renewedUntil`).
fn renewed_until(lease: &NodeLease, expires_at_millis: i64) -> NodeLease {
    let mut renewed = lease.clone();
    renewed.expires_at_millis = expires_at_millis;
    renewed.revoked = false;
    renewed
}
