//! A read or delete of another node's search, sent to that node.

use crate::jobs::Who;

/// What a relayed call asks of the node that runs the search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Read it, waiting for it when asked.
    Get,
    /// Cancel it or drop it.
    Delete,
}

/// One call to another node: the search's number there, the caller as this
/// node verified it, and the parameters already read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// Read or delete.
    pub op: Op,
    /// The search's number on its node.
    pub number: u64,
    /// The caller.
    pub who: Who,
    /// How long a read waits for a running search, in milliseconds.
    pub wait_ms: u64,
    /// The new keep-alive of a read, in milliseconds, when it sets one.
    pub keep_alive_ms: Option<u64>,
}

/// Why a call got no answer.
#[derive(Debug, PartialEq, Eq)]
pub enum RelayError {
    /// No live node has that id: nothing was sent.
    NoNode,
    /// The call failed on the way or the answer did not decode.
    Failed,
}

/// Sends a call to the node that runs the search.
pub trait Relay: Send + Sync {
    /// Sends `call` to node `node` and reads its status and body.
    ///
    /// # Errors
    ///
    /// [`RelayError`]: nothing sent, or the call failed.
    fn call(&self, node: &str, call: &Call) -> Result<(u16, Vec<u8>), RelayError>;
}
