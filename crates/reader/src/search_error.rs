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
        let busy = failure.kind == "extension_busy"
            || failure
                .cause
                .as_ref()
                .is_some_and(|c| c.kind == "extension_busy");
        let status = if busy {
            429
        } else if failure.kind == "extension_unavailable" {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn wrapped(cause: &'static str) -> Failure {
        Failure::opensearch("query_shard_exception", "failed to create query")
            .caused_by(Failure::plain(cause, "why"))
    }

    #[test]
    fn a_query_that_found_no_capacity_for_a_module_call_is_retryable() {
        assert_eq!(ShardError::of_query(wrapped("extension_busy")).status, 429);
        assert_eq!(
            ShardError::of_query(Failure::plain("extension_busy", "why")).status,
            429
        );
        assert_eq!(
            ShardError::of_query(Failure::plain("extension_unavailable", "why")).status,
            503
        );
        assert_eq!(
            ShardError::of_query(wrapped("extension_unavailable")).status,
            400,
            "an unavailable module under a wrapped failure answers as before"
        );
        assert_eq!(ShardError::of_query(wrapped("other")).status, 400);
    }
}
