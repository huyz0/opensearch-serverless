//! What the janitor needs of a node's write log and a shard's commit, which
//! live in the writer and the reader and so cannot be called from here.

use std::collections::BTreeMap;

use oss_format::{NodeLogEpoch, ShardKey};

use crate::JanitorError;

/// A dead node's write log with every epoch fenced: the node adds nothing
/// after what is read here.
pub trait FencedLog {
    /// Every shard the epochs' last headers name as holding records not yet
    /// published.
    ///
    /// # Errors
    ///
    /// [`JanitorError`] when a read fails.
    fn named(&self) -> Result<Vec<ShardKey>, JanitorError>;

    /// The highest sequence number of `shard`'s records in this log, from
    /// the blob headers alone, or a negative number when it holds none.
    ///
    /// # Errors
    ///
    /// [`JanitorError`] when a read fails.
    fn max_seq_no_of(&self, shard: &ShardKey) -> Result<i64, JanitorError>;

    /// Per epoch, the slot the fence took: every slot the node used is
    /// below it.
    ///
    /// # Errors
    ///
    /// [`JanitorError`] when a read fails.
    fn ends(&self) -> Result<BTreeMap<u64, u64>, JanitorError>;
}

/// The write logs of the nodes in a deployment.
pub trait NodeLogs {
    /// Fences every one of `epochs` of `node`'s log, so the node can add
    /// nothing after what is read, and answers what they hold. Called
    /// before anything is judged. An implementation may also check each
    /// fence against the lease as it stands afterwards; the collector's own
    /// guard is the lease swap that forgets the epochs, which a node that
    /// came back wins.
    ///
    /// # Errors
    ///
    /// [`JanitorError`] when a fence or a read fails.
    fn fence(
        &self,
        node: &str,
        epochs: &[NodeLogEpoch],
    ) -> Result<Box<dyn FencedLog + '_>, JanitorError>;

    /// Deletes slots `from..to` of `epoch` of `node`'s log, those that
    /// exist.
    ///
    /// # Errors
    ///
    /// [`JanitorError`] when a delete fails; slots before it are gone.
    fn delete_range(&self, node: &str, epoch: u64, from: u64, to: u64) -> Result<(), JanitorError>;
}

/// A shard's published commit, as far as the log's judgment needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Committed {
    /// No commit is published.
    Absent,
    /// A commit is published; its local checkpoint if it records one.
    Commit(Option<i64>),
}

/// The published commits of shards.
pub trait Commits {
    /// The commit of shard `shard` of incarnation `uuid` of `index`.
    ///
    /// # Errors
    ///
    /// [`JanitorError`] when the manifest or the commit cannot be read.
    fn committed(&self, index: &str, uuid: &str, shard: u32) -> Result<Committed, JanitorError>;
}
