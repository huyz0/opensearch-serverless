//! A response header that tells a request what its store requests cost,
//! when the request asked ([ADR 0061](../../../docs/adr/0061-a-request-reports-its-own-store-requests.md)).
//!
//! The server knows nothing of stores: a [`CostScope`] the node installs
//! begins a [`CostSession`] for a request that sent the request header, and
//! the session's value is written into the response head.

use std::sync::Arc;

/// The request header that asks for the cost, with the value `report`.
pub const REQUEST_HEADER: &str = "x-opensearch-serverless-cost";

/// Begins the accounting of one request that asked for it.
pub trait CostScope: Send + Sync {
    /// Starts counting for the request this worker thread is about to
    /// serve. Counting ends when the session drops, on the same thread.
    fn begin(&self) -> Arc<dyn CostSession>;
}

/// What one request has cost so far.
pub trait CostSession {
    /// The value of the response header, as of now.
    fn header_value(&self) -> String;
}

/// Whether a request header value asks for the report.
pub(crate) fn asks(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim().eq_ignore_ascii_case("report"))
}
