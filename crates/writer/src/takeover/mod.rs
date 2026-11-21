//! A node that won a shard's head brings the shard to a published commit
//! under its term: it replays what the shard's previous writers logged and
//! never published, adopts the result into the shard's register by
//! compare-and-swap, and stops the head naming the logs it replayed.
//!
//! Ported from `ShardReconciler`'s writer open and `NodeLogHooks` (huyz0/
//! OpenSearch `feature/serverlessshell`). Winning the head is
//! `oss_cluster::heads::ShardHeadStore::acquire`; this starts from what it
//! returned.

mod adopt;
mod checkpoint;
mod engine;
mod head;
mod predecessors;
mod restore;

pub use adopt::{AdoptError, Adopted, Engine, adopt};
pub use checkpoint::commit_checkpoint;
pub use engine::ShardEngine;
pub use head::{HeadWriters, Settled};
pub use predecessors::{Predecessors, PredecessorsFactory};
pub use restore::{RestoreError, Restored, restore};
