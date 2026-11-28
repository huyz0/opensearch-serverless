//! How a running search is told to give up: a question asked between shards.

use oss_reader::failure::{Failure, SearchError};

/// Asks whether a running search should give up: its job was cancelled or its
/// running time is spent. It is read between the shards of a search.
pub type Stop<'a> = &'a (dyn Fn() -> bool + Sync);

/// The error of a search that stopped at a shard boundary.
pub(super) fn cancelled() -> Box<SearchError> {
    // ALLOC: error path; one box.
    Box::new(SearchError::request(
        400,
        Failure::plain("task_cancelled_exception", "the search was cancelled"),
    ))
}
