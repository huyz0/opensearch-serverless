//! What the writer role of an assembled node is built on.

use std::sync::Arc;
use std::time::Duration;

use oss_writer::log::NodeLogCommitter;

use crate::writer::{Incarnation, Routes, ShardNames, ShardOpener};

/// What the writer role is built on.
pub struct Writer {
    /// The node's one write log.
    pub committer: Arc<NodeLogCommitter>,
    /// Which indexes exist and who owns a shard this node does not hold.
    pub routes: Arc<dyn Routes>,
    /// What opens a shard a write names and the node does not hold yet.
    pub opener: Option<Arc<dyn ShardOpener>>,
    /// The names the committer's admission finds a shard's head by, shared
    /// with it; none, and the role keeps its own.
    pub names: Option<Arc<ShardNames>>,
    /// Confirms an open shard's index is still the incarnation it was
    /// opened for, at most once a window.
    pub incarnation: Option<(Arc<dyn Incarnation>, Duration)>,
    /// Bucket members' heads and commits; none when buckets are off.
    pub buckets: Option<Arc<crate::writer::BucketWiring>>,
}
