//! What a bulk `update` item needs from the node: the document it merges
//! into or runs a script over, and the decision made of it. The writer
//! cannot depend on the coordinator that plans an update, so the node plugs
//! this in as it does for ingest pipelines.

use oss_rest::Exception;

use super::action::Action;
use super::run::IndexInfo;

/// What an update item became.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// Write `source`, creating the document when `create`; the write
    /// carries the sequence number and term just read.
    Put {
        /// The document to write.
        source: Vec<u8>,
        /// Whether the document must not exist.
        create: bool,
        /// `if_seq_no` the write requires.
        if_seq_no: i64,
        /// `if_primary_term` the write requires.
        if_primary_term: i64,
    },
    /// Delete the document read, at the sequence number just read.
    Delete {
        /// `if_seq_no` the delete requires.
        if_seq_no: i64,
        /// `if_primary_term` the delete requires.
        if_primary_term: i64,
    },
    /// Nothing changes.
    Noop {
        /// The document's `_version`.
        version: i64,
        /// Its `_seq_no`.
        seq_no: i64,
        /// Its `_primary_term`.
        primary_term: i64,
    },
}

/// Why an update item cannot go ahead: it fails alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateFailure {
    /// The item's HTTP status.
    pub status: u16,
    /// The failure, as `error`.
    pub error: Exception,
}

/// Reads and decides the update items of one bulk.
pub trait Updates {
    /// Reads the document `id` of `index`'s `shard` and decides what the
    /// update `body` makes of it.
    ///
    /// # Errors
    ///
    /// The failure of that item alone: a body that is not an update, a
    /// document missing, a condition that no longer holds, a script that
    /// fails, or a read that did not answer.
    fn resolve(
        &mut self,
        index: &IndexInfo,
        shard: u32,
        action: &Action,
        id: &str,
        body: &[u8],
    ) -> Result<Resolved, UpdateFailure>;
}
