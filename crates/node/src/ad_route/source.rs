//! Where a detector's windows are read from: the node's own search, behind a
//! seam so a test can answer without an index.

use oss_rest::RestError;
use serde_json::Value;

/// A search over the indexes a detector reads, answered as the search
/// response's JSON.
pub trait WindowSearch: Send + Sync {
    /// Runs `body` over `indices`.
    ///
    /// # Errors
    ///
    /// The search's refusal, such as a missing index or a full memory pool.
    fn search(&self, indices: &[String], body: &Value) -> Result<Value, RestError>;
}
