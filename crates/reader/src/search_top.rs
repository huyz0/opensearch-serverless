//! The top documents of a query on one shard: `TopDocsCollectorContext`'s
//! score and field collectors (server/src/main/java/org/opensearch/search/query/)
//! at OpenSearch 3.10.0, shared by a search's own hits and each `top_hits`.

use std::collections::HashMap;

use lucene_search::collector::{TopDocsCollector, TotalHitsRelation};
use lucene_search::field_norms::FieldNorms;
use lucene_search::index_searcher::IndexSearcher;
use lucene_search::multi_segment::{OpenSegment, search_boolean_query_multi_segment_min_score};
use lucene_search::terminate::search_sorted_until;
use lucene_search::top_field::{FieldDoc, SortField, search_sorted_min_score};
use lucene_search::{BooleanQuery, ScoreDoc, TotalHits};
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::search::{Hit, ShardError, SortValue, reported};
use crate::search_sort::KeyKind;

/// One shard's top documents for a query: the count and the hits after `from`.
pub(crate) struct Top {
    /// How many documents matched, exactly when `exact`.
    pub total: u64,
    /// Whether `total` is exact rather than a lower bound.
    pub exact: bool,
    /// Whether `terminate_after` ended the collection; `None` without it.
    pub terminated: Option<bool>,
    /// The best score of the window; `None` when sorted by fields.
    pub max_score: Option<f32>,
    /// The hits after `from`, at most the window less `from`.
    pub hits: Vec<Hit>,
}

/// What a search's collectors are asked besides the window.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TopOptions {
    /// `min_score`: `MinimumScoreCollector` around the collectors.
    pub min_score: Option<f32>,
    /// `track_scores`: a `MaxScoreCollector` beside a field sort's collector.
    pub track_scores: bool,
    /// `terminate_after`: `EarlyTerminatingCollector` in front of the
    /// collectors; 0 is off.
    pub terminate_after: u64,
}

impl TopOptions {
    /// What `source` asks of the collectors.
    pub(crate) fn of(source: &crate::dsl::SearchSource) -> Self {
        Self {
            min_score: source.min_score,
            track_scores: source.track_scores,
            terminate_after: source.terminate_after,
        }
    }
}

/// Collects the top `window` documents of `query` in `sort` (by score when
/// `None`), counting matches exactly up to `limit`, and drops the first
/// `from`. `after` resumes a sorted page.
#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_top<S: BlobStore + 'static>(
    segments: &[OpenSegment<'_>],
    shard: &OpenShard<S>,
    norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
    query: &BooleanQuery,
    sort: Option<&(Vec<SortField>, Vec<KeyKind>)>,
    window: usize,
    from: usize,
    limit: u64,
    after: Option<&FieldDoc>,
    options: TopOptions,
) -> Result<Top, ShardError> {
    if options.terminate_after > 0 {
        return collect_terminated(
            segments,
            shard,
            norms,
            query,
            (window, from, limit),
            options.terminate_after,
        );
    }
    match sort {
        Some((keys, kinds)) if window > 0 => {
            let top = search_sorted_min_score(
                segments,
                shard.reader.segment_readers(),
                query,
                norms,
                keys,
                window,
                limit,
                after,
                options.track_scores,
                &[],
                options.min_score,
            )
            .map_err(ShardError::engine)?;
            let max_score =
                (options.track_scores && !top.max_score.is_nan()).then_some(top.max_score);
            let score_at = kinds.iter().position(|k| *k == KeyKind::Score);
            let hits = top
                .hits
                .iter()
                .skip(from)
                .map(|h| sorted_hit(h, kinds, score_at))
                // ALLOC: one hit a result, at most MAX_RESULT_WINDOW.
                .collect();
            let (total, exact) = reported(top.total.value, top.total.relation, limit);
            Ok(Top {
                total,
                exact,
                terminated: None,
                max_score,
                hits,
            })
        }
        _ => {
            let scored;
            let mut collector = TopDocsCollector::with_total_hits_threshold(window, limit);
            let (top, counted): (&[ScoreDoc], TotalHits) = match options.min_score {
                Some(min) => {
                    let (docs, counted) = search_boolean_query_multi_segment_min_score(
                        segments, query, norms, window, limit, min,
                    )
                    .map_err(ShardError::engine)?;
                    scored = docs;
                    (&scored, counted)
                }
                None => {
                    let searcher =
                        IndexSearcher::new(segments, norms).map_err(ShardError::engine)?;
                    searcher
                        .search_collector(query, &mut collector)
                        .map_err(ShardError::engine)?;
                    (collector.top_docs(), collector.total_hits())
                }
            };
            let (total, exact) = reported(counted.value, counted.relation, limit);
            Ok(Top {
                total,
                exact,
                terminated: None,
                max_score: top.first().map(|h| h.score),
                hits: top
                    .iter()
                    .skip(from)
                    .map(|h| Hit {
                        doc: h.doc_id,
                        score: Some(h.score),
                        // ALLOC: empty.
                        sort: Vec::new(),
                    })
                    // ALLOC: one hit a result, at most MAX_RESULT_WINDOW.
                    .collect(),
            })
        }
    }
}

/// The top `window` of the first `n` matches in index order, by score:
/// `terminate_after`'s collector chain. The caller has refused what it does
/// not serve (a sort, `min_score`, a window of 0, a total below `n`).
fn collect_terminated<S: BlobStore + 'static>(
    segments: &[OpenSegment<'_>],
    shard: &OpenShard<S>,
    norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
    query: &BooleanQuery,
    (window, from, limit): (usize, usize, u64),
    n: u64,
) -> Result<Top, ShardError> {
    let keys = [SortField::score()];
    let (top, cut) = search_sorted_until(
        segments,
        shard.reader.segment_readers(),
        query,
        norms,
        &keys,
        window,
        None,
        false,
        n,
    )
    .map_err(ShardError::engine)?;
    let scored: Vec<Hit> = top
        .hits
        .iter()
        .map(|h| {
            let mut hit = sorted_hit(h, &[KeyKind::Score], Some(0));
            // ALLOC: empty; a search by score carries no sort values.
            hit.sort = Vec::new();
            hit
        })
        // ALLOC: one hit a result, at most MAX_RESULT_WINDOW.
        .collect();
    let max_score = scored.first().and_then(|h| h.score);
    let hits = scored.into_iter().skip(from).collect();
    let (total, exact) = reported(cut.collected, TotalHitsRelation::EqualTo, limit);
    Ok(Top {
        total,
        exact,
        terminated: Some(cut.terminated),
        max_score,
        hits,
    })
}

pub(crate) fn sorted_hit(h: &FieldDoc, kinds: &[KeyKind], score_at: Option<usize>) -> Hit {
    let value = |i: usize| h.values.get(i).copied().unwrap_or(0);
    let score_of = |i: usize| f32::from_bits(u32::try_from(value(i)).unwrap_or(0));
    let sort = kinds
        .iter()
        .enumerate()
        .map(|(i, kind)| match kind {
            KeyKind::Score => SortValue::Score(score_of(i)),
            KeyKind::Numeric => SortValue::Long(value(i)),
            KeyKind::Script => SortValue::Double(
                lucene_util::numeric_utils::sortable_long_to_double(value(i)),
            ),
            KeyKind::Term => SortValue::Term(h.terms.get(i).cloned().flatten()),
        })
        // ALLOC: one value a sort key, at most MAX_SORT_KEYS.
        .collect();
    Hit {
        doc: h.doc,
        score: score_at.map(score_of),
        sort,
    }
}

/// A field sort's hits carry no score; `track_scores` has each one scored,
/// as `TopFieldCollector.populateScores` does.
pub(crate) fn populate_scores(
    segments: &[OpenSegment<'_>],
    norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
    query: &BooleanQuery,
    sort: Option<&(Vec<SortField>, Vec<KeyKind>)>,
    top: &mut Top,
    track_scores: bool,
) -> Result<(), ShardError> {
    let Some((_, kinds)) = sort.filter(|_| track_scores) else {
        return Ok(());
    };
    if kinds.contains(&KeyKind::Score) || top.hits.is_empty() {
        return Ok(());
    }
    let searcher = IndexSearcher::new(segments, norms).map_err(ShardError::engine)?;
    let scored = crate::fetch::explain::explain_hits(&searcher, query, &top.hits)
        .map_err(ShardError::engine)?;
    for (hit, e) in top.hits.iter_mut().zip(&scored) {
        hit.score = Some(e.value);
    }
    Ok(())
}
