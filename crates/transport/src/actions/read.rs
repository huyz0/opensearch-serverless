//! The forwarded document read: `ForwardedGetRequest` and
//! `ForwardedGetResponse` as the Java shell writes them.
//!
//! A get must see writes that are acknowledged and not yet published, and
//! only the shard's owner holds those, so a get goes to the owner rather than
//! to a reader.

use crate::error::WireError;
use crate::stream::{Input, Output, TaskId};

/// `internal:serverless/document/get`.
pub const GET_ACTION: &str = "internal:serverless/document/get";

/// One document read, forwarded to the shard's owner (`ForwardedGetRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GetRequest {
    /// The parent task.
    pub parent: TaskId,
    /// The index name.
    pub index: String,
    /// The resolved index incarnation, or `None` to match by name.
    pub index_uuid: Option<String>,
    /// The shard the document routes to.
    pub shard: i32,
    /// The document id.
    pub id: String,
}

impl GetRequest {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.parent.write(out)?;
        out.string(&self.index)?;
        out.optional_string(self.index_uuid.as_deref())?;
        out.vint(self.shard)?;
        out.string(&self.id)
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a forwarded get";
        Ok(Self {
            parent: TaskId::read(input)?,
            index: input.string(WHAT)?,
            index_uuid: input.optional_string(WHAT)?,
            shard: input.vint(WHAT)?,
            id: input.string(WHAT)?,
        })
    }
}

/// What the owner found (`ForwardedGetResponse`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GetResponse {
    /// The node that performed the read.
    pub owner_node_id: String,
    /// The document id.
    pub id: String,
    /// Whether the document exists.
    pub found: bool,
    /// Its source, when found.
    pub source: Option<String>,
    /// Its sequence number.
    pub seq_no: i64,
    /// Its primary term.
    pub primary_term: i64,
    /// Its version.
    pub version: i64,
    /// Its custom routing value.
    pub routing: Option<String>,
}

impl GetResponse {
    /// `writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.string(&self.owner_node_id)?;
        out.string(&self.id)?;
        out.bool(self.found)?;
        out.optional_string(self.source.as_deref())?;
        out.zlong(self.seq_no)?;
        out.vlong(self.primary_term)?;
        out.zlong(self.version)?;
        out.optional_string(self.routing.as_deref())
    }

    /// The `StreamInput` constructor.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a forwarded get's answer";
        Ok(Self {
            owner_node_id: input.string(WHAT)?,
            id: input.string(WHAT)?,
            found: input.bool(WHAT)?,
            source: input.optional_string(WHAT)?,
            seq_no: input.zlong(WHAT)?,
            primary_term: input.vlong(WHAT)?,
            version: input.zlong(WHAT)?,
            routing: input.optional_string(WHAT)?,
        })
    }
}
