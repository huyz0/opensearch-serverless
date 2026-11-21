//! Publishing a shard's commit to the object store: one immutable blob per
//! segment, and a manifest in the shard's register, written only by
//! compare-and-swap so a replaced writer cannot be believed.
//!
//! Ported from `SegmentPublisher.publishCommit` (huyz0/OpenSearch
//! `feature/serverlessshell`) with its default settings: no staged
//! commits, no buckets, and node packs only when [`ShardPublisher::use_packs`] is given a
//! [`PackPlacement`] (default off).

mod blob;
mod error;
pub mod lazy;
mod names;
mod packed;
mod publisher;
mod register;

pub use error::PublishError;
pub use names::segment_name;
pub use packed::PackPlacement;
pub use publisher::{Published, ShardPublisher};
