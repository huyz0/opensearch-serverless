//! The forwarded writes: `ForwardedIndexRequest` and `ForwardedBulkRequest`
//! and their responses, as the Java shell writes them.
//!
//! The shard's owner answers these; a coordinator or a non-owning writer
//! sends them. Each round-trips byte for byte with the Java's `writeTo`,
//! because the request MAC digests the receiver's re-serialization.

use crate::error::WireError;
use crate::stream::{Input, Output, TaskId};

/// `internal:serverless/document/write`.
pub const INDEX_ACTION: &str = "internal:serverless/document/write";

/// `internal:serverless/document/bulk`.
pub const BULK_ACTION: &str = "internal:serverless/document/bulk";

/// `SequenceNumbers.UNASSIGNED_SEQ_NO`: no `if_seq_no` condition.
pub const UNASSIGNED_SEQ_NO: i64 = -2;

/// The fewest bytes one bulk operation takes on the wire.
const MIN_OPERATION_BYTES: usize = 7;

/// The fewest bytes one bulk outcome takes on the wire.
const MIN_OUTCOME_BYTES: usize = 10;

/// The most entries a decoder sizes for before reading them.
const PRESIZE_LIMIT: usize = 1024;

/// One document write or delete, forwarded to the shard's owner
/// (`ForwardedIndexRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexRequest {
    /// The parent task.
    pub parent: TaskId,
    /// The index name.
    pub index: String,
    /// The shard (a unit of the index's layout).
    pub shard: i32,
    /// The document id.
    pub id: String,
    /// The source JSON; empty for a deletion.
    pub source: String,
    /// Whether the write waits for a refresh.
    pub refresh: bool,
    /// A delete rather than an index.
    pub deletion: bool,
    /// The `if_seq_no` condition, [`UNASSIGNED_SEQ_NO`] for none.
    pub if_seq_no: i64,
    /// The `if_primary_term` condition, 0 for none.
    pub if_primary_term: i64,
    /// `op_type=create`: the id must not exist.
    pub require_absent: bool,
    /// The index's uuid, when the sender knows it.
    pub index_uuid: Option<String>,
    /// Sent by the owner-steering path rather than the coordinator.
    pub steered: bool,
    /// The custom routing value.
    pub routing: Option<String>,
}

impl IndexRequest {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.parent.write(out)?;
        out.string(&self.index)?;
        out.vint(self.shard)?;
        out.string(&self.id)?;
        out.string(&self.source)?;
        out.bool(self.refresh)?;
        out.bool(self.deletion)?;
        out.zlong(self.if_seq_no)?;
        out.vlong(self.if_primary_term)?;
        out.bool(self.require_absent)?;
        out.optional_string(self.index_uuid.as_deref())?;
        out.bool(self.steered)?;
        out.optional_string(self.routing.as_deref())
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a forwarded write";
        Ok(Self {
            parent: TaskId::read(input)?,
            index: input.string(WHAT)?,
            shard: input.vint(WHAT)?,
            id: input.string(WHAT)?,
            source: input.string(WHAT)?,
            refresh: input.bool(WHAT)?,
            deletion: input.bool(WHAT)?,
            if_seq_no: input.zlong(WHAT)?,
            if_primary_term: input.vlong(WHAT)?,
            require_absent: input.bool(WHAT)?,
            index_uuid: input.optional_string(WHAT)?,
            steered: input.bool(WHAT)?,
            routing: input.optional_string(WHAT)?,
        })
    }
}

/// The owner's answer to a forwarded write (`ForwardedIndexResponse`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexResponse {
    /// The node that applied it.
    pub owner_node_id: String,
    /// The operation's sequence number.
    pub seq_no: i64,
    /// Its primary term.
    pub primary_term: i64,
    /// The document's version after it.
    pub version: i64,
    /// An index that created the document.
    pub created: bool,
    /// A delete that found the document.
    pub found: bool,
    /// Whether a refresh made it visible before the answer.
    pub refreshed: bool,
}

impl IndexResponse {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.string(&self.owner_node_id)?;
        out.zlong(self.seq_no)?;
        out.vlong(self.primary_term)?;
        out.zlong(self.version)?;
        out.bool(self.created)?;
        out.bool(self.found)?;
        out.bool(self.refreshed)
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a forwarded write's answer";
        Ok(Self {
            owner_node_id: input.string(WHAT)?,
            seq_no: input.zlong(WHAT)?,
            primary_term: input.vlong(WHAT)?,
            version: input.zlong(WHAT)?,
            created: input.bool(WHAT)?,
            found: input.bool(WHAT)?,
            refreshed: input.bool(WHAT)?,
        })
    }
}

/// One operation of a forwarded bulk (`BulkOperation` and its `WalRecord`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BulkOperation<'a> {
    /// The document id.
    pub id: String,
    /// The source JSON; a deletion's is written and read as empty. Borrowed
    /// by a request built from items already held, owned by one decoded.
    pub source: std::borrow::Cow<'a, str>,
    /// A delete rather than an index.
    pub deletion: bool,
    /// The `if_seq_no` condition, [`UNASSIGNED_SEQ_NO`] for none.
    pub if_seq_no: i64,
    /// The `if_primary_term` condition, 0 for none.
    pub if_primary_term: i64,
    /// `op_type=create`.
    pub require_absent: bool,
    /// The custom routing value.
    pub routing: Option<String>,
}

/// A shard's slice of a bulk, forwarded to its owner (`ForwardedBulkRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BulkRequest<'a> {
    /// The parent task.
    pub parent: TaskId,
    /// The index name.
    pub index: String,
    /// The shard.
    pub shard: i32,
    /// The operations, in order.
    pub operations: Vec<BulkOperation<'a>>,
    /// Whether the bulk waits for a refresh.
    pub refresh: bool,
    /// The index's uuid, when the sender knows it.
    pub index_uuid: Option<String>,
    /// Sent by the owner-steering path.
    pub steered: bool,
    /// Whether the owner pushes the tail to readers for this bulk.
    pub tail_push: bool,
}

impl<'a> BulkRequest<'a> {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.parent.write(out)?;
        out.string(&self.index)?;
        out.vint(self.shard)?;
        out.count(self.operations.len())?;
        for op in &self.operations {
            out.string(&op.id)?;
            // The Java rebuilds a deletion without its source and digests
            // what it rebuilt: a source sent with one would fail the MAC.
            out.string(if op.deletion { "" } else { op.source.as_ref() })?;
            out.bool(op.deletion)?;
            out.zlong(op.if_seq_no)?;
            out.vlong(op.if_primary_term)?;
            out.bool(op.require_absent)?;
            out.optional_string(op.routing.as_deref())?;
        }
        out.bool(self.refresh)?;
        out.optional_string(self.index_uuid.as_deref())?;
        out.bool(self.steered)?;
        out.bool(self.tail_push)
    }

    /// The same request owning every string, to keep past the items it
    /// borrowed from.
    pub fn into_owned(self) -> BulkRequest<'static> {
        BulkRequest {
            operations: self
                .operations
                .into_iter()
                .map(|op| BulkOperation {
                    id: op.id,
                    source: std::borrow::Cow::Owned(op.source.into_owned()),
                    deletion: op.deletion,
                    if_seq_no: op.if_seq_no,
                    if_primary_term: op.if_primary_term,
                    require_absent: op.require_absent,
                    routing: op.routing,
                })
                // ALLOC: one entry an operation, the request's own count.
                .collect(),
            parent: self.parent,
            index: self.index,
            shard: self.shard,
            refresh: self.refresh,
            index_uuid: self.index_uuid,
            steered: self.steered,
            tail_push: self.tail_push,
        }
    }

    /// At least how many bytes [`BulkRequest::write`] writes, so a buffer
    /// of this capacity is filled without growing.
    pub fn encoded_size_hint(&self) -> usize {
        // Per operation: a length for the id and the source (5 each at
        // most), the three flags and conditions (at most 1 + 10 + 10 + 1),
        // and the routing's presence byte.
        const PER_OPERATION: usize = 5 + 5 + 1 + 10 + 10 + 1 + 1;
        let operations = self.operations.iter().fold(0usize, |n, op| {
            let source = if op.deletion { 0 } else { op.source.len() };
            n.saturating_add(PER_OPERATION)
                .saturating_add(op.id.len())
                .saturating_add(source)
                .saturating_add(op.routing.as_ref().map_or(0, |r| r.len().saturating_add(5)))
        });
        // The parent, index, uuid and flags are small beside the operations.
        operations
            .saturating_add(self.index.len())
            .saturating_add(self.parent.task.as_ref().map_or(0, |(node, _)| node.len()))
            .saturating_add(self.index_uuid.as_ref().map_or(0, String::len))
            .saturating_add(128)
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes; [`WireError::Truncated`] for a count of
    /// operations the bytes left cannot hold.
    pub fn read(input: &mut Input<'_>) -> Result<BulkRequest<'static>, WireError> {
        const WHAT: &str = "a forwarded bulk";
        let parent = TaskId::read(input)?;
        let index = input.string(WHAT)?;
        let shard = input.vint(WHAT)?;
        let count = input.count(MIN_OPERATION_BYTES, WHAT)?;
        // A guarded handler checks the MAC before this decodes.
        // ALLOC: one entry per operation read, about 100 bytes each and at
        // most one per 7 bytes of the frame; pre-sized to PRESIZE_LIMIT.
        let mut operations = Vec::with_capacity(count.min(PRESIZE_LIMIT));
        for _ in 0..count {
            let id = input.string(WHAT)?;
            let source = input.string(WHAT)?;
            let deletion = input.bool(WHAT)?;
            // ALLOC: as above.
            operations.push(BulkOperation {
                id,
                // `WalRecord.deletion(id)`: a deletion has no source.
                // ALLOC: none; an empty string holds nothing.
                source: std::borrow::Cow::Owned(if deletion { String::new() } else { source }),
                deletion,
                if_seq_no: input.zlong(WHAT)?,
                if_primary_term: input.vlong(WHAT)?,
                require_absent: input.bool(WHAT)?,
                routing: input.optional_string(WHAT)?,
            });
        }
        Ok(BulkRequest {
            parent,
            index,
            shard,
            operations,
            refresh: input.bool(WHAT)?,
            index_uuid: input.optional_string(WHAT)?,
            steered: input.bool(WHAT)?,
            tail_push: input.bool(WHAT)?,
        })
    }
}

/// One operation's outcome (`BulkOutcome`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BulkOutcome {
    /// The document id.
    pub id: String,
    /// Why it failed, or `None` when it was applied.
    pub failure: Option<String>,
    /// A lost `if_seq_no` or `op_type=create` condition (409).
    pub conflict: bool,
    /// Mapping contention: the caller may retry.
    pub retryable: bool,
    /// A delete.
    pub deletion: bool,
    /// A delete that found the document.
    pub found: bool,
    /// The operation's sequence number.
    pub seq_no: i64,
    /// Its primary term.
    pub primary_term: i64,
    /// The document's version after it.
    pub version: i64,
    /// An index that created the document.
    pub created: bool,
}

/// The owner's answer to a forwarded bulk (`ForwardedBulkResponse`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BulkResponse {
    /// The node that applied it.
    pub owner_node_id: String,
    /// One outcome per operation, in order.
    pub outcomes: Vec<BulkOutcome>,
    /// Whether a refresh made it visible before the answer.
    pub refreshed: bool,
}

impl BulkResponse {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.string(&self.owner_node_id)?;
        out.count(self.outcomes.len())?;
        for o in &self.outcomes {
            out.string(&o.id)?;
            out.optional_string(o.failure.as_deref())?;
            out.bool(o.conflict)?;
            out.bool(o.retryable)?;
            out.bool(o.deletion)?;
            out.bool(o.found)?;
            out.zlong(o.seq_no)?;
            out.vlong(o.primary_term)?;
            out.zlong(o.version)?;
            out.bool(o.created)?;
        }
        out.bool(self.refreshed)
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes; [`WireError::Truncated`] for a count of
    /// outcomes the bytes left cannot hold.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a forwarded bulk's answer";
        let owner_node_id = input.string(WHAT)?;
        let count = input.count(MIN_OUTCOME_BYTES, WHAT)?;
        // ALLOC: one entry per outcome read, about 100 bytes each and at
        // most one per 10 bytes of the frame, from a peer this node called;
        // pre-sized to at most PRESIZE_LIMIT.
        let mut outcomes = Vec::with_capacity(count.min(PRESIZE_LIMIT));
        for _ in 0..count {
            outcomes.push(BulkOutcome {
                id: input.string(WHAT)?,
                failure: input.optional_string(WHAT)?,
                conflict: input.bool(WHAT)?,
                retryable: input.bool(WHAT)?,
                deletion: input.bool(WHAT)?,
                found: input.bool(WHAT)?,
                seq_no: input.zlong(WHAT)?,
                primary_term: input.vlong(WHAT)?,
                version: input.zlong(WHAT)?,
                created: input.bool(WHAT)?,
            });
        }
        Ok(Self {
            owner_node_id,
            outcomes,
            refreshed: input.bool(WHAT)?,
        })
    }
}
