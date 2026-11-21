//! The janitor's two seams over a dead node's write log and a shard's
//! published commit, read from the store.
//!
//! The janitor never depends on the writer, so what it needs of a log (fence
//! its epochs, name what they hold, delete a range) and of a commit (whether
//! it holds everything the log does) comes in through [`NodeLogs`] and
//! [`Commits`]; this implements them over the writer's log reader.
//!
//! Ported from `ShardSettlement.collectNodeLog`'s reads (the fences, the
//! headers' shard names, the commit's local checkpoint).

use std::collections::BTreeMap;
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_format::{NodeLogEpoch, ShardKey, ShardRef};
use oss_janitor::JanitorError;
use oss_janitor::node_log::{Commits, Committed, FencedLog, NodeLogs};
use oss_writer::log::{NodeLogStore, NodeLogTakeover};
use oss_writer::takeover::{Predecessors, commit_checkpoint};

use super::predecessors::lease_epochs;

fn log_error(e: impl std::fmt::Display) -> JanitorError {
    // ALLOC: error path; one message.
    JanitorError::Log(e.to_string())
}

/// Dead nodes' write logs, fenced as node `fenced_by` at `term`.
pub struct StoreNodeLogs {
    store: Arc<dyn BlobStore>,
    base: String,
    fenced_by: String,
    term: i64,
    shared: Option<Arc<Predecessors>>,
}

impl StoreNodeLogs {
    /// Logs under `base` in `store`, fenced by node `fenced_by` at `term`
    /// (recorded in each fence for diagnosis).
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>, base: &str, fenced_by: &str, term: i64) -> Self {
        Self {
            store,
            // ALLOC: two names, once a node.
            base: base.to_owned(),
            fenced_by: fenced_by.to_owned(),
            term,
            shared: None,
        }
    }

    /// Fences through `predecessors`, the readers the node's takes and its
    /// proactive pass share, so a dead node's log is fenced once however it
    /// is reached.
    #[must_use]
    pub fn sharing(mut self, predecessors: Arc<Predecessors>) -> Self {
        self.shared = Some(predecessors);
        self
    }

    /// Drops the shared reader of `node` once its log is collected, so a
    /// node that returns under the same id is read afresh.
    pub fn forget(&self, node: &str) {
        if let Some(shared) = &self.shared {
            shared.forget(node);
        }
    }

    fn log(&self, node: &str) -> Result<NodeLogStore, JanitorError> {
        NodeLogStore::new(Arc::clone(&self.store), self.base.clone(), node).map_err(log_error)
    }
}

/// A fenced log: what the takeover reader found.
struct Fenced(Arc<NodeLogTakeover>);

impl FencedLog for Fenced {
    fn named(&self) -> Result<Vec<ShardKey>, JanitorError> {
        self.0
            .named()
            .map(|named| named.into_iter().collect())
            .map_err(log_error)
    }

    fn max_seq_no_of(&self, shard: &ShardKey) -> Result<i64, JanitorError> {
        self.0.max_seq_no_of(shard).map_err(log_error)
    }

    fn ends(&self) -> Result<BTreeMap<u64, u64>, JanitorError> {
        self.0.ends().map_err(log_error)
    }
}

impl NodeLogs for StoreNodeLogs {
    fn fence(
        &self,
        node: &str,
        epochs: &[NodeLogEpoch],
    ) -> Result<Box<dyn FencedLog + '_>, JanitorError> {
        if let Some(shared) = &self.shared
            && let Some(takeover) = shared.of_naming(node, epochs).map_err(log_error)?
        {
            takeover.fence_all().map_err(log_error)?;
            return Ok(Box::new(Fenced(takeover)));
        }
        let store = Arc::clone(&self.store);
        let base = self.base.clone();
        // ALLOC: one name, once a log fenced.
        let again = node.to_owned();
        let takeover = NodeLogTakeover::rereading(
            self.log(node)?,
            epochs.to_vec(),
            &self.fenced_by,
            self.term,
            Box::new(move || Ok(lease_epochs(store.as_ref(), &base, &again)?.unwrap_or_default())),
        );
        takeover.fence_all().map_err(log_error)?;
        Ok(Box::new(Fenced(Arc::new(takeover))))
    }

    fn delete_range(&self, node: &str, epoch: u64, from: u64, to: u64) -> Result<(), JanitorError> {
        self.log(node)?
            .delete_range(epoch, from, to)
            .map_err(log_error)
    }
}

/// Shards' published commits, read from their registers.
pub struct StoreCommits {
    store: Arc<dyn BlobStore>,
    base: String,
}

impl StoreCommits {
    /// Commits under `base` in `store`.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Self {
        Self {
            store,
            // ALLOC: one name, once a node.
            base: base.to_owned(),
        }
    }
}

impl Commits for StoreCommits {
    fn committed(&self, index: &str, uuid: &str, shard: u32) -> Result<Committed, JanitorError> {
        let at = ShardRef { index, uuid, shard };
        Ok(
            match commit_checkpoint(self.store.as_ref(), &self.base, &at).map_err(log_error)? {
                None => Committed::Absent,
                Some(checkpoint) => Committed::Commit(checkpoint),
            },
        )
    }
}
