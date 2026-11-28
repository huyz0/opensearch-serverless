//! An asynchronous search read or delete forwarded to the node that runs the
//! search.
//!
//! A search's result lives in the memory of the node that took its submit,
//! so a `GET` or `DELETE` that reaches another node is sent to the one the id
//! names, carrying the caller's tenant and user: the node that runs the
//! search answers only that caller. The call is this project's own: no Java
//! node sends or answers it.

use crate::error::WireError;
use crate::stream::{Input, Output};

/// `internal:serverless/async_search/call`.
pub const ASYNC_SEARCH_ACTION: &str = "internal:serverless/async_search/call";

/// The most bytes of an answer's body: a result of at most 1 MiB and its
/// envelope.
const MAX_BODY: usize = 2 << 20;

/// What a call asks of the search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Read it, waiting for it when asked.
    Get,
    /// Cancel it or drop it.
    Delete,
}

/// A read or a delete of one search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsyncSearchRequest {
    /// Read or delete.
    pub op: Op,
    /// The search's number on the node that runs it.
    pub number: u64,
    /// The caller's tenant, as the first node verified it.
    pub tenant: String,
    /// The caller's user, as the first node verified it.
    pub user: String,
    /// How long a read waits for a running search, in milliseconds.
    pub wait_ms: u64,
    /// The new keep-alive of a read, in milliseconds, when it sets one.
    pub keep_alive_ms: Option<u64>,
}

fn long(value: u64, what: &'static str) -> Result<i64, WireError> {
    i64::try_from(value).map_err(|_| WireError::malformed(what, "a number past i64"))
}

impl AsyncSearchRequest {
    /// Writes the request.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit; malformed for a
    /// number past `i64`.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        const WHAT: &str = "an asynchronous search request";
        out.byte(match self.op {
            Op::Get => 0,
            Op::Delete => 1,
        })?;
        out.vlong(long(self.number, WHAT)?)?;
        out.string(&self.tenant)?;
        out.string(&self.user)?;
        out.vlong(long(self.wait_ms, WHAT)?)?;
        out.bool(self.keep_alive_ms.is_some())?;
        out.vlong(long(self.keep_alive_ms.unwrap_or(0), WHAT)?)
    }

    /// Reads a request.
    ///
    /// # Errors
    ///
    /// As the reads it makes; an unknown op or a negative number is malformed.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "an asynchronous search request";
        let op = match input.byte(WHAT)? {
            0 => Op::Get,
            1 => Op::Delete,
            _ => return Err(WireError::malformed(WHAT, "an unknown op")),
        };
        let unsigned =
            |n: i64| u64::try_from(n).map_err(|_| WireError::malformed(WHAT, "a negative number"));
        let number = unsigned(input.vlong(WHAT)?)?;
        let tenant = input.string(WHAT)?;
        let user = input.string(WHAT)?;
        let wait_ms = unsigned(input.vlong(WHAT)?)?;
        let has_keep_alive = input.bool(WHAT)?;
        let keep_alive = unsigned(input.vlong(WHAT)?)?;
        Ok(Self {
            op,
            number,
            tenant,
            user,
            wait_ms,
            keep_alive_ms: has_keep_alive.then_some(keep_alive),
        })
    }
}

/// The REST answer the node that runs the search gave.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AsyncSearchResponse {
    /// The HTTP status.
    pub status: u16,
    /// The JSON body.
    pub body: String,
}

impl AsyncSearchResponse {
    /// Writes the answer.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit or `MAX_BODY`.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        if self.body.len() > MAX_BODY {
            return Err(WireError::too_large(
                "an asynchronous search answer",
                MAX_BODY,
                self.body.len(),
            ));
        }
        out.vint(i32::from(self.status))?;
        out.string(&self.body)
    }

    /// Reads an answer.
    ///
    /// # Errors
    ///
    /// As the reads it makes; a status outside HTTP's is malformed.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "an asynchronous search answer";
        let status = u16::try_from(input.vint(WHAT)?)
            .map_err(|_| WireError::malformed(WHAT, "a status past HTTP's"))?;
        if !(100..600).contains(&status) {
            return Err(WireError::malformed(WHAT, "a status past HTTP's"));
        }
        Ok(Self {
            status,
            body: input.string(WHAT)?,
        })
    }
}
