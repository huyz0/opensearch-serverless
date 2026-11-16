//! OpenSearch's `StreamInput` and `StreamOutput` encodings, and the small
//! values every transport message carries: versions and task ids.

mod input;
mod output;

pub use input::Input;
pub use output::Output;

use crate::error::WireError;

/// A parent task (`TaskId`): the node id and the task's number there, or
/// none. Every transport request begins with one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskId {
    /// The node and task, or `None` for the empty task id.
    pub task: Option<(String, i64)>,
}

impl TaskId {
    /// The empty task id: a request with no parent.
    pub const EMPTY: Self = Self { task: None };

    /// `TaskId.writeTo`: the node id, then the number unless it is empty.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        match &self.task {
            None => out.string(""),
            Some((node, id)) => {
                out.string(node)?;
                out.long(*id)
            }
        }
    }

    /// `TaskId.readFromStream`.
    ///
    /// # Errors
    ///
    /// As [`Input::string`] and [`Input::long`].
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        let node = input.string("a parent task id")?;
        if node.is_empty() {
            return Ok(Self::EMPTY);
        }
        let id = input.long("a parent task id")?;
        Ok(Self {
            task: Some((node, id)),
        })
    }
}
