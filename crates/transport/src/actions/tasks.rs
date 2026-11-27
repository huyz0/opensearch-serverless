//! A task read or cancel forwarded to the node that runs the task.
//!
//! A task lives in the memory of the node that took its request, so a
//! `GET /_tasks/{id}` that reaches another node is sent to the one the id
//! names. The call is this project's own: no Java node sends or answers it.

use crate::error::WireError;
use crate::stream::{Input, Output};

/// `internal:serverless/tasks/call`.
pub const TASKS_ACTION: &str = "internal:serverless/tasks/call";

/// The most bytes of an answer's body.
const MAX_BODY: usize = 1 << 17;

/// A read or a cancel of one task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskRequest {
    /// Cancel the task rather than read it.
    pub cancel: bool,
    /// The task's number on the node that runs it.
    pub number: u64,
}

impl TaskRequest {
    /// Writes the request.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.bool(self.cancel)?;
        out.vlong(
            i64::try_from(self.number)
                .map_err(|_| WireError::malformed("a task request", "the number is past i64"))?,
        )
    }

    /// Reads a request.
    ///
    /// # Errors
    ///
    /// As the reads it makes; a negative number is malformed.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a task request";
        let cancel = input.bool(WHAT)?;
        let number = u64::try_from(input.vlong(WHAT)?)
            .map_err(|_| WireError::malformed(WHAT, "a negative number"))?;
        Ok(Self { cancel, number })
    }
}

/// The REST answer the node that runs the task gave.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskResponse {
    /// The HTTP status.
    pub status: u16,
    /// The JSON body.
    pub body: String,
}

impl TaskResponse {
    /// Writes the answer.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit or `MAX_BODY`.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        if self.body.len() > MAX_BODY {
            return Err(WireError::too_large(
                "a task answer",
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
        const WHAT: &str = "a task answer";
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
