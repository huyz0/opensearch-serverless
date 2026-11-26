//! Adopting a shard won from a predecessor.

use std::fmt;

use oss_blob::BlobError;
use oss_format::{FormatError, ShardHead, ShardKey, WalRecord};

use super::{HeadWriters, Predecessors, Settled};
use crate::log::TakeoverError;
use crate::publish::{PublishError, Published};

/// What adopting needs of the shard: the commit it opened from, a way to
/// apply a record, and a publish.
pub trait Engine {
    /// The highest sequence number the opened commit holds; records at or
    /// below it are not applied.
    fn committed_through(&self) -> i64;

    /// Applies a record of a predecessor's log; `false` when it was stale
    /// and changed nothing.
    ///
    /// # Errors
    ///
    /// The record or the engine refusing it, as a message.
    fn replay(&mut self, record: &WalRecord<'_>) -> Result<bool, String>;

    /// Whether a commit adopted from a predecessor's log waits to be written
    /// to the register by the next publish, even when nothing is replayed.
    fn adopted_commit(&self) -> bool {
        false
    }

    /// The bytes the engine holds for what it applied and has not flushed.
    fn buffered_bytes(&self) -> usize;

    /// Commits what was applied locally, releasing what it buffered.
    ///
    /// # Errors
    ///
    /// The commit failing, as a message.
    fn flush(&mut self) -> Result<(), String>;

    /// Publishes everything applied as a commit of `term`, swapping the
    /// register.
    ///
    /// # Errors
    ///
    /// As [`crate::publish::ShardPublisher::publish`].
    fn publish(&mut self, term: u64) -> Result<Published, PublishError>;
}

/// What adopting a shard did.
#[derive(Debug)]
pub struct Adopted {
    /// Records applied to the engine.
    pub replayed: u64,
    /// The publish that put them in the register; `None` when none was
    /// applied.
    pub published: Option<Published>,
    /// What clearing the head's writers came to.
    pub settled: Settled,
}

/// Why adopting a shard failed; the shard is not served from it, and a
/// later attempt starts again.
#[derive(Debug)]
pub enum AdoptError {
    /// A predecessor's log could not be fenced or read.
    Log(TakeoverError),
    /// A record could not be applied.
    Replay(String),
    /// The publish was refused or failed.
    Publish(PublishError),
    /// The head could not be read or written.
    Head(BlobError),
    /// The head does not parse or does not fit.
    HeadFormat(FormatError),
    /// The head's term is not a term a commit can carry.
    BadTerm(i64),
}

impl fmt::Display for AdoptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Log(e) => write!(f, "replaying: {e}"),
            Self::Replay(e) => write!(f, "applying a record: {e}"),
            Self::Publish(e) => write!(f, "publishing the replay: {e}"),
            Self::Head(e) => write!(f, "the shard head: {e}"),
            Self::HeadFormat(e) => write!(f, "the shard head: {e}"),
            Self::BadTerm(t) => write!(f, "the shard head's term {t} is not a term"),
        }
    }
}

impl std::error::Error for AdoptError {}

impl From<TakeoverError> for AdoptError {
    fn from(e: TakeoverError) -> Self {
        Self::Log(e)
    }
}

/// Brings `shard`, whose head this node won as `won`, to a published
/// commit: every record the head's writers logged and no commit holds is
/// applied oldest first, published once under `won`'s term if any was, and
/// only then are the writers whose logs were read cleared from the head.
///
/// The engine is flushed whenever its buffer has grown by `max_buffer_bytes`
/// (at least 1) since the last flush left it, so a replay of any length
/// holds that, what a flush leaves behind, and one record, and always
/// makes progress even when a flush leaves more than the bound.
///
/// # Errors
///
/// A predecessor's log not reading, a record not applying, the publish
/// refused (a newer term holds the register) or failing, or the head not
/// reading or writing. The head keeps naming its writers on every error.
pub fn adopt(
    shard: &ShardKey,
    won: &ShardHead,
    predecessors: &Predecessors,
    engine: &mut dyn Engine,
    heads: &HeadWriters,
    max_buffer_bytes: usize,
) -> Result<Adopted, AdoptError> {
    let through = engine.committed_through();
    let mut replayed = 0u64;
    let growth = max_buffer_bytes.max(1);
    // What the engine held after its last flush.
    let mut floor = 0usize;
    // The writers whose logs were read: a writer with no lease is not known
    // to have logged nothing, so the head keeps naming it.
    // ALLOC: the writers a head names, at most MAX_WRITERS.
    let mut read: Vec<String> = Vec::new();
    for writer in &won.writers {
        let Some(log) = predecessors.of(writer)? else {
            continue;
        };
        log.for_each_record_above(shard, through, |record| {
            if record.has_sequence_identity() && record.seq_no() <= through {
                return Ok(());
            }
            // Bounded however long the replay: the buffer is released
            // before the record that would pass the bound.
            if engine.buffered_bytes() >= floor.saturating_add(growth) {
                engine.flush().map_err(AdoptError::Replay)?;
                floor = engine.buffered_bytes();
            }
            if engine.replay(&record).map_err(AdoptError::Replay)? {
                replayed = replayed.saturating_add(1);
            }
            Ok::<_, AdoptError>(())
        })?;
        // ALLOC: a writer's name, once a take.
        read.push(writer.clone());
    }
    let published = if replayed == 0 && !engine.adopted_commit() {
        None
    } else {
        let term = u64::try_from(won.term).map_err(|_| AdoptError::BadTerm(won.term))?;
        Some(engine.publish(term).map_err(AdoptError::Publish)?)
    };
    let settled = if read.is_empty() {
        Settled::NothingToClear
    } else {
        heads.settle(won, &read)?
    };
    Ok(Adopted {
        replayed,
        published,
        settled,
    })
}
