//! The reader pushes: `CommitPushRequest` and its answer, and
//! `NewestCommitRequest` and its answer, as the Java shell writes them.
//!
//! A writer sends the commit push to the search nodes of a shard it just
//! published; a reader that finds a staged register sends the newest-commit
//! ask to the writer. Each round-trips byte for byte with the Java's
//! `writeTo`, because the request MAC digests the receiver's
//! re-serialization. The commit travels as the bytes the writer wrote (the
//! register's value), kept as read: a manifest written out again need not
//! come out byte for byte the same.

use crate::error::WireError;
use crate::stream::{Input, Output, TaskId};

/// `internal:serverless/commit/push`.
pub const PUSH_ACTION: &str = "internal:serverless/commit/push";

/// `internal:serverless/commit/newest`.
pub const NEWEST_ACTION: &str = "internal:serverless/commit/newest";

/// The fewest bytes one block takes on the wire: a one-byte key length, a
/// one-byte length.
const MIN_BLOCK_BYTES: usize = 2;

/// The most entries a decoder sizes for before reading them.
const PRESIZE_LIMIT: usize = 64;

/// A commit pushed to a search node (`CommitPushRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushRequest {
    /// The parent task.
    pub parent: TaskId,
    /// The index name.
    pub index_name: String,
    /// The index's uuid.
    pub index_uuid: String,
    /// The shard number.
    pub shard: i32,
    /// The commit register's value, as the writer wrote it.
    pub commit: Vec<u8>,
    /// Blocks of the commit's new files, by cache key: empty when none ride
    /// along.
    pub blocks: Vec<(String, Vec<u8>)>,
    /// The mapping version of the descriptor the writer holds, or -1.
    pub mapping_version: i64,
    /// The settings version of the descriptor the writer holds, or -1.
    pub settings_version: i64,
}

impl PushRequest {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.parent.write(out)?;
        out.string(&self.index_name)?;
        out.string(&self.index_uuid)?;
        out.vint(self.shard)?;
        out.bytes_reference(&self.commit)?;
        out.count(self.blocks.len())?;
        for (key, block) in &self.blocks {
            out.string(key)?;
            out.bytes_reference(block)?;
        }
        out.zlong(self.mapping_version)?;
        out.zlong(self.settings_version)
    }

    /// The `StreamInput` constructor. The commit and the blocks are kept as
    /// read; whether the commit parses is the receiver's to find out.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a commit push";
        let parent = TaskId::read(input)?;
        let index_name = input.string(WHAT)?;
        let index_uuid = input.string(WHAT)?;
        let shard = input.vint(WHAT)?;
        // ALLOC: the commit register's value, at most the frame.
        let commit = input.bytes_reference(WHAT)?.to_vec();
        let count = input.count(MIN_BLOCK_BYTES, WHAT)?;
        // A guarded handler checks the MAC before this decodes.
        // ALLOC: one entry per block read, at most one per 2 bytes of the
        // frame; pre-sized to PRESIZE_LIMIT.
        let mut blocks = Vec::with_capacity(count.min(PRESIZE_LIMIT));
        for _ in 0..count {
            let key = input.string(WHAT)?;
            // ALLOC: one block, at most the frame.
            blocks.push((key, input.bytes_reference(WHAT)?.to_vec()));
        }
        Ok(Self {
            parent,
            index_name,
            index_uuid,
            shard,
            commit,
            blocks,
            mapping_version: input.zlong(WHAT)?,
            settings_version: input.zlong(WHAT)?,
        })
    }
}

/// A search node's answer to a push (`CommitPushResponse`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PushResponse {
    /// Whether the shard is open on the search node as a reader.
    pub open: bool,
}

impl PushResponse {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.bool(self.open)
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the read it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        Ok(Self {
            open: input.bool("a commit push's answer")?,
        })
    }
}

/// A reader's ask of a shard's writer for its newest staged commit
/// (`NewestCommitRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewestRequest {
    /// The parent task.
    pub parent: TaskId,
    /// The index name.
    pub index_name: String,
    /// The index's uuid.
    pub index_uuid: String,
    /// The shard number.
    pub shard: i32,
}

impl NewestRequest {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.parent.write(out)?;
        out.string(&self.index_name)?;
        out.string(&self.index_uuid)?;
        out.vint(self.shard)
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a newest-commit ask";
        Ok(Self {
            parent: TaskId::read(input)?,
            index_name: input.string(WHAT)?,
            index_uuid: input.string(WHAT)?,
            shard: input.vint(WHAT)?,
        })
    }
}

/// The writer's answer (`NewestCommitResponse`): its newest staged commit's
/// register value, or none.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewestResponse {
    /// The commit register's value, as the writer wrote it.
    pub commit: Option<Vec<u8>>,
}

impl NewestResponse {
    /// `writeTo`: `writeOptionalBytesReference`, a length plus one, or zero
    /// for none.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        match &self.commit {
            None => out.count(0),
            Some(bytes) => {
                out.count(bytes.len().saturating_add(1))?;
                out.bytes(bytes)
            }
        }
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a newest-commit answer";
        let wire = input.count(0, WHAT)?;
        let Some(len) = wire.checked_sub(1) else {
            return Ok(Self { commit: None });
        };
        // ALLOC: the commit register's value, at most the frame.
        Ok(Self {
            commit: Some(input.take(len, WHAT)?.to_vec()),
        })
    }
}
