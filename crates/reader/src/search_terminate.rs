//! What `terminate_after` is served for.

use lucene_search::{BooleanQuery, Clause};

use crate::dsl::SearchSource;
use crate::failure::Failure;
use crate::search::ShardError;

/// Refuses what `terminate_after` is not served for: a collector chain this
/// node does not reproduce (its cut falls where the engine's batching puts it
/// only for a scored query collected by score, over a window).
pub(crate) fn refusal(
    source: &SearchSource,
    query: &BooleanQuery,
    sorted: bool,
) -> Result<(), ShardError> {
    let not_served = |what: &str| {
        ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("[terminate_after] with {what} is not supported by this node yet"),
        ))
    };
    if source.size == 0 {
        return Err(not_served("a size of 0"));
    }
    if source.min_score.is_some() {
        return Err(not_served("min_score"));
    }
    if source.track_scores {
        return Err(not_served("track_scores"));
    }
    if sorted {
        return Err(not_served("a sort"));
    }
    if !source.search_after.is_empty() {
        return Err(not_served("search_after"));
    }
    if !source.aggs.is_empty() {
        return Err(not_served("aggregations"));
    }
    if source.track_total_hits.unwrap_or(0) < source.terminate_after {
        return Err(not_served("a track_total_hits below it"));
    }
    // A query with nothing to score (a filter, a constant score, a match-all)
    // is collected by ranges, which cuts elsewhere.
    let scoring = !query.must.is_empty() || !query.should.is_empty();
    let constant = matches!(
        query.lone_clause(),
        Some(Clause::ConstantScore(_) | Clause::MatchAllDocs(_))
    );
    if !scoring || constant {
        return Err(not_served("a query that does not score"));
    }
    Ok(())
}
