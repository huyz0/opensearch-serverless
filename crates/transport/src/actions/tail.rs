//! The tail overlay's messages: `TailFetchRequest` and its answer, and
//! `TailPushRequest` and its answer, as the Java shell writes them.
//!
//! A reader that opens a shard fetches the writes its writer acknowledged
//! past the commit it opened, and is then pushed the writes that follow. Each
//! message round-trips byte for byte with the Java's `writeTo`, because the
//! request MAC digests the receiver's re-serialization. Records travel in
//! the shell's `TailRecords` form.

use crate::error::WireError;
use crate::stream::{Input, Output, TaskId};

/// `internal:serverless/tail/fetch`.
pub const FETCH_ACTION: &str = "internal:serverless/tail/fetch";

/// `internal:serverless/tail/push`.
pub const PUSH_ACTION: &str = "internal:serverless/tail/push";

/// A fetch's subscription asking to be pushed to anew, with what is kept past
/// the commit (`TailFetchRequest.NEW`).
pub const NEW: i64 = -1;

/// A fetch's subscription asking only how far refreshing writes have been
/// answered (`TailFetchRequest.CHECK`).
pub const CHECK: i64 = -2;

/// The subscription of an answer that subscribed nothing
/// (`TailFetchResponse.NONE`).
pub const NONE: i64 = -1;

/// The fewest bytes one record takes: a one-byte id length, the deletion
/// flag, a one-byte source length, three one-byte numbers, the routing flag.
const MIN_RECORD_BYTES: usize = 7;

/// The fewest bytes one shard takes: two one-byte string lengths, a one-byte
/// shard number.
const MIN_SHARD_BYTES: usize = 3;

/// The most entries a decoder sizes for before reading them.
const PRESIZE_LIMIT: usize = 64;

/// One acknowledged write (`TailRecords`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Record {
    /// The document id.
    pub id: String,
    /// Whether the write deleted the document.
    pub deletion: bool,
    /// The document's source; empty for a deletion.
    pub source: String,
    /// The sequence number.
    pub seq_no: i64,
    /// The primary term.
    pub primary_term: i64,
    /// The version.
    pub version: i64,
    /// The routing, if the write had one.
    pub routing: Option<String>,
}

impl Record {
    fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.string(&self.id)?;
        out.bool(self.deletion)?;
        // A deletion has no source on the wire.
        out.string(if self.deletion { "" } else { &self.source })?;
        out.zlong(self.seq_no)?;
        out.zlong(self.primary_term)?;
        out.zlong(self.version)?;
        out.optional_string(self.routing.as_deref())
    }

    fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a tail record";
        Ok(Self {
            id: input.string(WHAT)?,
            deletion: input.bool(WHAT)?,
            source: input.string(WHAT)?,
            seq_no: input.zlong(WHAT)?,
            primary_term: input.zlong(WHAT)?,
            version: input.zlong(WHAT)?,
            routing: input.optional_string(WHAT)?,
        })
    }
}

fn write_records(out: &mut Output<'_>, records: &[Record]) -> Result<(), WireError> {
    out.count(records.len())?;
    records.iter().try_for_each(|record| record.write(out))
}

fn read_records(input: &mut Input<'_>, what: &'static str) -> Result<Vec<Record>, WireError> {
    let count = input.count(MIN_RECORD_BYTES, what)?;
    // A guarded handler checks the MAC before this decodes.
    // ALLOC: one entry per record read, at most one per 7 bytes of the frame;
    // pre-sized to PRESIZE_LIMIT.
    let mut records = Vec::with_capacity(count.min(PRESIZE_LIMIT));
    for _ in 0..count {
        records.push(Record::read(input)?);
    }
    Ok(records)
}

/// A shard named as the Java writes a `ShardId`: index name, uuid, number.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShardRef {
    /// The index name.
    pub index_name: String,
    /// The index's uuid.
    pub index_uuid: String,
    /// The shard number.
    pub shard: i32,
}

impl ShardRef {
    fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.string(&self.index_name)?;
        out.string(&self.index_uuid)?;
        out.vint(self.shard)
    }

    fn read(input: &mut Input<'_>, what: &'static str) -> Result<Self, WireError> {
        Ok(Self {
            index_name: input.string(what)?,
            index_uuid: input.string(what)?,
            shard: input.vint(what)?,
        })
    }
}

/// A reader's ask of a shard's writer for what it acknowledged past a commit
/// (`TailFetchRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FetchRequest {
    /// The parent task.
    pub parent: TaskId,
    /// The shard.
    pub shard: ShardRef,
    /// The commit's local checkpoint: only records above it are wanted.
    pub after_seq_no: i64,
    /// The asking node, which the writer pushes the shard's next writes to.
    pub subscriber: String,
    /// The subscription held and to be renewed, [`NEW`] or [`CHECK`].
    pub subscription: i64,
}

impl FetchRequest {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.parent.write(out)?;
        self.shard.write(out)?;
        out.zlong(self.after_seq_no)?;
        out.string(&self.subscriber)?;
        out.zlong(self.subscription)
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a tail fetch";
        Ok(Self {
            parent: TaskId::read(input)?,
            shard: ShardRef::read(input, WHAT)?,
            after_seq_no: input.zlong(WHAT)?,
            subscriber: input.string(WHAT)?,
            subscription: input.zlong(WHAT)?,
        })
    }
}

/// The writer's answer to a fetch (`TailFetchResponse`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResponse {
    /// The records above the asked commit.
    pub records: Vec<Record>,
    /// The subscription the asking node is pushed under, or [`NONE`].
    pub subscription: i64,
    /// Whether the records, with the asking node's commit, hold every write
    /// the writer acknowledged.
    pub complete: bool,
    /// The highest sequence number a refreshing write may have been answered
    /// as refreshed through; `i64::MAX` when the answering node cannot say.
    pub claimed_through: i64,
}

impl Default for FetchResponse {
    fn default() -> Self {
        Self {
            // ALLOC: empty, holds no memory
            records: Vec::new(),
            subscription: NONE,
            complete: false,
            claimed_through: -1,
        }
    }
}

impl FetchResponse {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        write_records(out, &self.records)?;
        out.zlong(self.subscription)?;
        out.bool(self.complete)?;
        out.zlong(self.claimed_through)
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a tail fetch's answer";
        Ok(Self {
            records: read_records(input, WHAT)?,
            subscription: input.zlong(WHAT)?,
            complete: input.bool(WHAT)?,
            claimed_through: input.zlong(WHAT)?,
        })
    }
}

/// Acknowledged writes pushed to a search node, by shard
/// (`TailPushRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushRequest {
    /// The parent task.
    pub parent: TaskId,
    /// The records, by shard, in the order written.
    pub shards: Vec<(ShardRef, Vec<Record>)>,
}

impl PushRequest {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.parent.write(out)?;
        out.count(self.shards.len())?;
        for (shard, records) in &self.shards {
            shard.write(out)?;
            write_records(out, records)?;
        }
        Ok(())
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a tail push";
        let parent = TaskId::read(input)?;
        // A shard takes its names, a number and a record count.
        let count = input.count(MIN_SHARD_BYTES.saturating_add(1), WHAT)?;
        // ALLOC: one entry per shard read, at most one per 4 bytes of the
        // frame; pre-sized to PRESIZE_LIMIT.
        let mut shards = Vec::with_capacity(count.min(PRESIZE_LIMIT));
        for _ in 0..count {
            let shard = ShardRef::read(input, WHAT)?;
            shards.push((shard, read_records(input, WHAT)?));
        }
        Ok(Self { parent, shards })
    }
}

/// A search node's answer to a push (`TailPushResponse`): the shards it does
/// not want pushed any more.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushResponse {
    /// The shards not wanted.
    pub unwanted: Vec<ShardRef>,
}

impl PushResponse {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.count(self.unwanted.len())?;
        self.unwanted.iter().try_for_each(|shard| shard.write(out))
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a tail push's answer";
        let count = input.count(MIN_SHARD_BYTES, WHAT)?;
        // ALLOC: one entry per shard read, at most one per 3 bytes of the
        // frame; pre-sized to PRESIZE_LIMIT.
        let mut unwanted = Vec::with_capacity(count.min(PRESIZE_LIMIT));
        for _ in 0..count {
            unwanted.push(ShardRef::read(input, WHAT)?);
        }
        Ok(Self { unwanted })
    }
}
