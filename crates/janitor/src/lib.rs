//! The cleanup role: reclaims what nothing names any more.
//!
//! Ported from the shell's `reconcile` package (`GarbageCollector`,
//! `JanitorPass`) at `3920dc36`. The janitor deletes, so everything here
//! errs toward keeping: a blob goes only on the rules in [`gc`], after two
//! sightings, and a pass deletes unless `serverless.janitor.dry_run` ([`settings::DRY_RUN`]) is set.
//!
//! - [`gc`]: the sweep of one shard's segment blobs.
//! - [`settings`]: the janitor's node settings.

pub mod gc;
pub mod node_log;
pub mod orphans;
pub mod published;
pub mod settings;

mod error;

pub use error::JanitorError;
