//! Fleet membership on registers.
//!
//! A node is a member while its own lease register says so: written only by
//! that node, renewed by compare-and-swap, and read by every peer. No
//! cluster manager, lock or leader decides who is in; safety comes from the
//! shard heads' compare-and-swap, and membership only says who to ask.
//! Rust and Java nodes on one bucket see each other, as the Java shell's
//! `membership` package does.
//!
//! - [`Membership`]: renew, revoke, release and refresh leases, with the
//!   members index and the lease digest that keep a refresh to a read or
//!   two however large the fleet.
//! - [`LivenessOracle`]: whether a node is live, by a fresh read of its
//!   lease, and revocation of a dead one shared among concurrent takeovers.
//! - [`placement`]: the owner of a shard's writes, the readers of its
//!   searches and the winner of a takeover, computed alike by every node
//!   from the leases, with the Java's hashes.
//! - [`EarlyRevoke`]: revoking a refused peer's lease before it expires,
//!   off by default (`serverless.lease.early_revoke`).
//! - [`zones`]: a node's zone from its awareness attribute, the zone
//!   preference that keeps a hot shard's searches in the coordinator's zone,
//!   and the meter of bytes exchanged across zones.
//! - [`transport_secret`]: the keys of the transport's request MAC, read
//!   from their register and cached for one lease TTL.

pub mod lease;
pub use lease::early_revoke;
pub mod placement;
pub mod routing;
pub mod transport_secret;
pub mod zones;

mod error;

pub use error::ClusterError;
pub use lease::{
    ConnectProbe, EarlyRevoke, LeaseClock, LeaseConfig, LivenessOracle, Membership,
    MembershipDelta, Refreshed, SystemClock, TcpProbe, Verdict,
};
