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

pub mod lease;
pub mod placement;

mod error;

pub use error::ClusterError;
pub use lease::{
    LeaseClock, LeaseConfig, LivenessOracle, Membership, MembershipDelta, Refreshed, SystemClock,
};
