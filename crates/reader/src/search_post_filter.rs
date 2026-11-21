//! `post_filter`: the hits' query is the query and the filter, scored by the
//! query alone; the aggregations keep the query.

use lucene_search::{BooleanQuery, Clause};

use crate::dsl::SearchSource;
use crate::failure::Failure;
use crate::search::ShardError;

/// `query` with `filter` required and unscored.
pub(crate) fn narrowed(query: &BooleanQuery, filter: BooleanQuery) -> BooleanQuery {
    BooleanQuery {
        // ALLOC: one clause, once a search.
        must: vec![Clause::Boolean(Box::new(query.clone()))],
        // ALLOC: one clause, once a search.
        filter: vec![Clause::Boolean(Box::new(filter))],
        ..BooleanQuery::default()
    }
}

/// Refuses what a `post_filter` is not served beside: the collectors that
/// wrap or follow it differently in OpenSearch, and the explanation of a
/// query that now holds the filter.
pub(crate) fn refusal(source: &SearchSource) -> Result<(), ShardError> {
    let not_served = |what: &str| {
        ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("[post_filter] with {what} is not supported by this node yet"),
        ))
    };
    if source.min_score.is_some() {
        return Err(not_served("min_score"));
    }
    if source.terminate_after > 0 {
        return Err(not_served("terminate_after"));
    }
    if source.explain {
        return Err(not_served("explain"));
    }
    if source.track_scores {
        return Err(not_served("track_scores"));
    }
    Ok(())
}
