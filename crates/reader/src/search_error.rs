//! The failure a search of one shard ends in.

use crate::failure::Failure;

/// A failure on the shard, with the HTTP status it answers with.
#[derive(Clone, Debug, PartialEq)]
pub struct ShardError {
    /// The status.
    pub status: u16,
    /// The exception.
    pub failure: Failure,
}

impl ShardError {
    pub(crate) fn bad_request(failure: Failure) -> Self {
        Self {
            status: 400,
            failure,
        }
    }

    /// A filter or an aggregation's queries that did not plan: the client's
    /// fault, except an `AggregationExecutionException`, which stock answers
    /// as a server error.
    pub(crate) fn of_plan(failure: Failure) -> Self {
        let status = if failure.kind == "aggregation_execution_exception" {
            500
        } else {
            400
        };
        Self { status, failure }
    }

    /// A query that did not build: the client's fault, except a pinned
    /// analyzer that could not run, which is the node's and is retried.
    pub(crate) fn of_query(failure: Failure) -> Self {
        let status = if failure.kind == "extension_unavailable" {
            503
        } else {
            400
        };
        Self { status, failure }
    }

    pub(crate) fn engine(e: impl std::fmt::Display) -> Self {
        // ALLOC: error path; one message.
        let message = e.to_string();
        if let Some(failure) = crate::script_failure::of_message(&message) {
            return Self::bad_request(failure);
        }
        Self {
            status: 500,
            // ALLOC: error path; one message.
            failure: Failure::opensearch("exception", message),
        }
    }
}
