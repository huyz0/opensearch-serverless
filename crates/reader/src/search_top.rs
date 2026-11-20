//! The top documents of a query on one shard: `TopDocsCollectorContext`'s
//! score and field collectors (server/src/main/java/org/opensearch/search/query/)
//! at OpenSearch 3.10.0, shared by a search's own hits and each `top_hits`.

use std::collections::HashMap;

use lucene_search::collector::TopDocsCollector;
use lucene_search::field_norms::FieldNorms;
use lucene_search::index_searcher::IndexSearcher;
use lucene_search::multi_segment::{OpenSegment, search_boolean_query_multi_segment_min_score};
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
                false,
                &[],
                options.min_score,
            )
            .map_err(ShardError::engine)?;
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
                max_score: None,
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

fn sorted_hit(h: &FieldDoc, kinds: &[KeyKind], score_at: Option<usize>) -> Hit {
    let value = |i: usize| h.values.get(i).copied().unwrap_or(0);
    let score_of = |i: usize| f32::from_bits(u32::try_from(value(i)).unwrap_or(0));
    let sort = kinds
        .iter()
        .enumerate()
        .map(|(i, kind)| match kind {
            KeyKind::Score => SortValue::Score(score_of(i)),
            KeyKind::Numeric => SortValue::Long(value(i)),
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
