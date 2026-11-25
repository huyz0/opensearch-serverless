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

/// `index.max_result_window`: refuses a window of `from + size` hits past
/// `limit`, the index's own.
pub(crate) fn check_window(window: usize, limit: usize) -> Result<(), ShardError> {
    if window > limit {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "Result window is too large, from + size must be less than or equal to: \
                 [{limit}] but was [{window}]. See the scroll api for a more \
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

#[cfg(test)]
mod tests {
    use super::check_window;

    // docs/specs/index-max-result-window AC-2, AC-3
    #[test]
    fn a_window_past_the_indexs_limit_is_refused_naming_that_limit() {
        assert!(check_window(50, 50).is_ok());
        let e = check_window(51, 50).expect_err("refused");
        let reason = e.failure.reason;
        assert!(
            reason.contains("less than or equal to: [50] but was [51]"),
            "{reason}"
        );
        assert!(reason.contains("[index.max_result_window]"), "{reason}");
        assert!(check_window(10_000, 10_000).is_ok());
        assert!(check_window(10_001, 10_000).is_err());
    }
}
