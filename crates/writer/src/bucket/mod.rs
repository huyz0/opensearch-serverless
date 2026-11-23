//! Members' commits in their bucket's one manifest register.
//!
//! Ported from the shell's `BucketCommits` and the register half of
//! `BucketStore` (huyz0/OpenSearch `feature/serverlessshell`). Small indices
//! are grouped in a bucket whose register `buckets/<id>/manifest` holds
//! every member's latest commit; a node checkpoints as many members as are
//! ready with one compare-and-swap, judging each against its own entry's
//! version rather than the register's generation.

mod assign;
mod commits;
mod error;
pub mod policy;
pub mod settings;
mod store;

pub use assign::{BucketAssigner, Limits, STRIPES};
pub use commits::{BucketCommits, MemberCommit, MemberOutcome, Swap};
pub use error::BucketError;
pub use store::{BucketRead, BucketStore, ManifestWrite, OpenRead, Swapped};
