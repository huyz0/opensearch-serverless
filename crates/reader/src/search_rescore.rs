//! `rescore` on one shard: `RescoreProcessor` and `QueryRescorer`
//! (server/src/main/java/org/opensearch/search/rescore/) at OpenSearch
//! 3.10.0, over the engine's `opensearch_query_rescore`.

use std::collections::HashMap;

use lucene_search::index_searcher::IndexSearcher;
use lucene_search::multi_segment::OpenSegment;
use lucene_search::rescorer::{QueryRescoreMode, opensearch_query_rescore};
use lucene_search::top_docs::ShardScoreDoc;
use lucene_search::values_source::ValuesContext;
use lucene_search::{BooleanQuery, field_norms::FieldNorms};
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::dsl::SearchSource;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, shard_failure, to_lucene};
use crate::search::{Hit, ShardError, scored_fields};
use crate::search_top::Top;

/// One rescorer, its query built.
struct Built {
    query: BooleanQuery,
    window: usize,
    query_weight: f32,
    rescore_query_weight: f32,
    mode: QueryRescoreMode,
}

/// One step of the rescore: a Lucene query, or an `sltr` model.
enum Step {
    Query(Built),
    Ltr(crate::search_ltr::Built),
}

impl Step {
    fn window(&self) -> usize {
        match self {
            Self::Query(b) => b.window,
            Self::Ltr(b) => b.window(),
        }
    }
}

/// A search's rescorers.
pub(crate) struct Plan {
    built: Vec<Step>,
}

/// `DefaultSearchContext.preProcess`'s rescore checks, and each rescore
/// query as a Lucene query; `None` for a search without `rescore`.
pub(crate) fn plan<S: BlobStore + 'static>(
    source: &SearchSource,
    cx: &QueryContext<'_>,
    shard: &OpenShard<S>,
    index: &str,
    sorted: bool,
) -> Result<Option<Plan>, ShardError> {
    if source.rescore.is_empty() {
        return Ok(None);
    }
    let refused = |reason: String| {
        ShardError::bad_request(Failure::plain("illegal_argument_exception", reason))
    };
    if sorted {
        return Err(refused(
            // ALLOC: error path; one message.
            "Cannot use [sort] option in conjunction with [rescore].".to_owned(),
        ));
    }
    let max = shard.meta.max_result_window();
    // ALLOC: one entry a rescorer, bounded by the body's recording.
    let mut built = Vec::with_capacity(source.rescore.len());
    for r in &source.rescore {
        if r.window > max {
            // ALLOC: error path; one message.
            return Err(refused(format!(
                // ALLOC: error path; one message.
                "Rescore window [{}] is too large. It must be less than [{max}]. This prevents allocating massive heaps for storing the results to be rescored. This limit can be set by changing the [index.max_rescore_window] index level setting.",
                r.window
            )));
        }
        if let crate::dsl::query::Query::More(m) = &r.query
            && let crate::dsl::queries::More::Sltr(sltr) = m.as_ref()
        {
            built.push(Step::Ltr(crate::search_ltr::build(
                r, sltr, cx, shard, index,
            )?));
            continue;
        }
        let query = to_lucene(&r.query, cx)
            .map_err(|f| ShardError::of_query(shard_failure(f, index, &shard.meta.uuid)))?;
        built.push(Step::Query(Built {
            query,
            window: r.window,
            query_weight: r.query_weight,
            rescore_query_weight: r.rescore_query_weight,
            mode: r.mode,
        }));
    }
    Ok(Some(Plan { built }))
}

impl Plan {
    /// Whether a rescore query needs the points open.
    pub(crate) fn has_points(&self) -> bool {
        self.built.iter().any(|step| match step {
            Step::Query(b) => crate::search::has_points(&b.query),
            Step::Ltr(b) => b.has_points(),
        })
    }

    /// The window the first pass collects and the `from` it drops:
    /// `max(from + size, every window)` hits, none dropped yet.
    pub(crate) fn collection(&self, window: usize) -> (usize, usize) {
        (self.built.iter().fold(window, |w, b| w.max(b.window())), 0)
    }

    /// Rescores `top`'s hits in order, then drops the first `from` and keeps
    /// the `window` less `from` after.
    pub(crate) fn apply<S: BlobStore + 'static>(
        &self,
        shard: &OpenShard<S>,
        segments: &[OpenSegment<'_>],
        top: &mut Top,
        (from, window): (usize, usize),
    ) -> Result<(), ShardError> {
        if top.hits.is_empty() {
            return Ok(());
        }
        // ALLOC: one name a scored field, bounded by the rescore queries.
        let mut fields: Vec<String> = Vec::new();
        for b in self.built.iter().filter_map(|s| match s {
            Step::Query(b) => Some(b),
            Step::Ltr(_) => None,
        }) {
            for f in scored_fields(&b.query) {
                if !fields.contains(&f) {
                    fields.push(f);
                }
            }
        }
        let owned = shard
            .reader
            .field_norms_by_field(&fields)
            .map_err(ShardError::engine)?;
        // ALLOC: one entry a segment.
        let norms: Vec<Option<&HashMap<String, FieldNorms<'_>>>> = owned.iter().map(Some).collect();
        let searcher = IndexSearcher::new(segments, &norms).map_err(ShardError::engine)?;
        let ctx = ValuesContext::new(&searcher);
        // ALLOC: one entry a collected hit, at most the largest window.
        let mut hits: Vec<ShardScoreDoc> = top
            .hits
            .iter()
            .map(|h| ShardScoreDoc::new(h.doc, h.score.unwrap_or(0.0)))
            // ALLOC: one hit a result, at most MAX_RESULT_WINDOW.
            .collect();
        for step in &self.built {
            match step {
                Step::Query(b) => {
                    opensearch_query_rescore(
                        &ctx,
                        &mut hits,
                        b.query.clone(),
                        b.window,
                        b.query_weight,
                        b.rescore_query_weight,
                        b.mode,
                    )
                    .map_err(ShardError::engine)?;
                }
                Step::Ltr(b) => b.apply(&ctx, &mut hits)?,
            }
        }
        top.max_score = hits.first().map(|h| h.score);
        top.hits = hits
            .iter()
            .skip(from)
            .take(window.saturating_sub(from))
            .map(|h| Hit {
                doc: h.doc,
                score: Some(h.score),
                // ALLOC: empty; a search by score carries no sort values.
                sort: Vec::new(),
            })
            // ALLOC: one hit a result, at most MAX_RESULT_WINDOW.
            .collect();
        Ok(())
    }
}

/// The `search_after` key of a sorted page, none without one.
pub(crate) fn after_key(
    source: &SearchSource,
    mapping: &crate::mapping::Mapping,
    sort: Option<&(
        Vec<lucene_search::top_field::SortField>,
        Vec<crate::search_sort::KeyKind>,
    )>,
) -> Result<Option<lucene_search::top_field::FieldDoc>, ShardError> {
    use crate::dsl::search_after::{AfterKey, field_doc};
    if source.search_after.is_empty() {
        return Ok(None);
    }
    let is_date = |name: &str| {
        matches!(
            mapping.get(name).map(|f| &f.ty),
            Some(crate::mapping::FieldType::Date)
        )
    };
    let keys: Option<Vec<AfterKey<'_>>> = sort.map(|(fields, kinds)| {
        fields
            .iter()
            .zip(kinds)
            .map(|(f, kind)| match (f.ty, kind) {
                // A `half_float` sort's comparator is OpenSearch's own; its key is a float.
                (
                    lucene_search::top_field::SortType::Custom(_),
                    crate::search_sort::KeyKind::Float,
                ) => AfterKey::Float(f.field.as_str()),
                (
                    lucene_search::top_field::SortType::Custom(_),
                    crate::search_sort::KeyKind::Double,
                ) => AfterKey::Double(f.field.as_str()),
                _ => AfterKey::of(f, is_date),
            })
            // ALLOC: one key a sort key, at most MAX_SORT_KEYS.
            .collect()
    });
    Ok(Some(field_doc(
        source.from,
        &source.search_after,
        keys.as_deref(),
    )?))
}
