//! Publishing a shard's commit to the object store: one immutable blob per
//! segment, and a manifest in the shard's register, written only by
//! compare-and-swap so a replaced writer cannot be believed.
//!
//! Ported from `SegmentPublisher.publishCommit` (huyz0/OpenSearch
//! `feature/serverlessshell`) with its default settings: no staged
//! commits, no node packs, no buckets.

mod blob;
mod error;
mod names;
mod publisher;

pub use error::PublishError;
pub use names::segment_name;
pub use publisher::{Published, ShardPublisher};
