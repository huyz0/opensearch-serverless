//! The search body's options this node serves only in some combinations.

use lucene_search::BooleanQuery;

use crate::dsl::SearchSource;
use crate::failure::Failure;
use crate::fetch::explain;
use crate::search::ShardError;
use crate::search_sort::KeyKind;
use crate::{search_post_filter, search_terminate};

fn refused(reason: String) -> ShardError {
    ShardError::bad_request(Failure::plain("illegal_argument_exception", reason))
}

/// `index.max_result_window`: refuses a window of `from + size` hits past it.
pub(crate) fn check_window(window: usize) -> Result<(), ShardError> {
    use crate::search::MAX_RESULT_WINDOW;
    if window > MAX_RESULT_WINDOW {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "Result window is too large, from + size must be less than or equal to: \
                 [{MAX_RESULT_WINDOW}] but was [{window}]. See the scroll api for a more \
                 efficient way to request large data sets. This limit can be set by changing \
                 the [index.max_result_window] index level setting."
            ),
        )));
    }
    Ok(())
}

/// Refuses `min_score`, `track_scores`, `terminate_after` and `post_filter`
/// where they meet something this node does not serve with them.
pub(crate) fn check(
    source: &SearchSource,
    query: &BooleanQuery,
    kinds: Option<&[KeyKind]>,
) -> Result<(), ShardError> {
    if source.min_score.is_some() && !source.aggs.is_empty() {
        // The collector wraps the aggregations' too; this node's do not.
        return Err(refused(
            // ALLOC: error path; one message.
            "[min_score] with aggregations is not supported by this node yet".to_owned(),
        ));
    }
    if source.terminate_after > 0 {
        search_terminate::refusal(source, query, kinds.is_some())?;
    }
    if source.post_filter.is_some() {
        search_post_filter::refusal(source)?;
    }
    if source.track_scores
        && let Some(kinds) = kinds
    {
        if kinds.first() == Some(&KeyKind::Score) {
            return Err(refused(
                // ALLOC: error path; one message.
                "[track_scores] with a sort led by the score is not supported by this node yet"
                    .to_owned(),
            ));
        }
        if !kinds.contains(&KeyKind::Score)
            && let Some(what) = explain::refusal(query)
        {
            // ALLOC: error path; one message.
            return Err(refused(format!(
                "[track_scores] of {what} is not supported by this node yet"
            )));
        }
    }
    Ok(())
}
