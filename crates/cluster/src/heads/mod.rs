//! Shard heads: who owns a shard, and at what term.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ShardHeadStore.java`
//! (`read`, `acquire` of an unbucketed shard) and `RegisterMap.shardHeadBlob`.
//! The head is the one object that arbitrates ownership: a node existing is
//! not a node owning anything. It lives at
//! `shards/<2 hex digits>-<index>#<shard>`, the digits the low byte of the
//! murmur3 of `<index>#<shard>`, so heads spread over a bucket's key space.

mod bucket;
mod registers;
mod store;

pub use bucket::{BucketHeads, Member, bucket_head_index};
pub use registers::{HeadAt, HeadRegisters};
pub use store::{Acquisition, ShardHeadStore};

use crate::placement::murmur3;

/// Where under a deployment's base the heads are.
pub const HEADS_PREFIX: &str = "shards/";

/// The key of a shard's head under `base` (empty, or ending in `/`).
pub fn shard_head_key(base: &str, index: &str, shard: u32) -> String {
    // ALLOC: one name a head operation.
    let name = format!("{index}#{shard}");
    // The low byte of the hash, as the Java's `hash & 0xFF`.
    let [low, ..] = murmur3(&[&name]).to_le_bytes();
    // ALLOC: one key a head operation, the length of its parts.
    format!("{base}{HEADS_PREFIX}{low:02x}-{name}")
}
