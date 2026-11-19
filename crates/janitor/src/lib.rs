//! The cleanup role: reclaims what nothing names any more.
//!
//! Ported from the shell's `reconcile` package (`GarbageCollector`,
//! `JanitorPass`) at `3920dc36`. The janitor deletes, so everything here
//! errs toward keeping: a blob goes only on the rules in [`gc`], after two
//! sightings, and a pass starts as a dry run ([`settings::DRY_RUN`]).
//!
//! - [`gc`]: the sweep of one shard's segment blobs.
//! - [`settings`]: the janitor's node settings.

pub mod gc;
pub mod node_log;
pub mod settings;

mod error;

pub use error::JanitorError;
