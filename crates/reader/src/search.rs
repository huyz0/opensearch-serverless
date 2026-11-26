//! Running a search on one open shard: the query phase.
//!
//! Ported from `QueryPhase.executeInternal` and `TopDocsCollectorContext`
//! (server/src/main/java/org/opensearch/search/query/), the result window
//! check of `DefaultSearchContext.preProcess`
//! (server/src/main/java/org/opensearch/search/DefaultSearchContext.java),
//! and `SortBuilder.buildSort`, `FieldSortBuilder.build` and
//! `ScoreSortBuilder.build`
//! (server/src/main/java/org/opensearch/search/sort/), at OpenSearch 3.10.0.
//! The top `from + size` hits are collected, counting matches exactly up to
//! `track_total_hits`' default; the first `from` are then skipped. A sort
//! that is only `_score` descending is no sort.

use std::collections::HashMap;

use lucene_analysis::Analyzer;
use lucene_search::collector::TotalHitsRelation;
use lucene_search::extended_query::ExtendedQuery;
use lucene_search::field_norms::FieldNorms;
use lucene_search::index_searcher::IndexSearcher;
use lucene_search::top_field::FieldDoc;
use lucene_search::{BooleanQuery, Clause, Explanation};
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::composite_collect;
use crate::aggs::bucket::filters_collect;
use crate::aggs::bucket::{fixed_subs, keyed_subs, keyword_collect, keyword_subs};
use crate::aggs::metric::{self, Partial, state_bytes};
use crate::aggs::metric::{top_hits, top_hits_sub};
use crate::dsl::search_after::{AfterKey, field_doc};
use crate::dsl::{SearchSource, SortKey};
use crate::failure::Failure;
use crate::fetch::docvalue_fields::{self, HitField};
use crate::fetch::explain;
use crate::fetch::highlight::{self, Plan};
use crate::fetch::hit_meta::HitMeta;
use crate::fetch::source_filter::SourceFilter;
use crate::lucene_query::{QueryContext, shard_failure, to_lucene};
use crate::mapping::FieldType;
use crate::search_options;
use crate::search_post_filter;
use crate::search_sort::{KeyKind, sort_fields};
use crate::search_top::{TopOptions, collect_top};

/// `index.max_result_window`'s default: the most `from + size` may reach.
pub const MAX_RESULT_WINDOW: usize = 10_000;

/// `track_total_hits`' default: matches are counted exactly up to here.
pub const TRACK_TOTAL_HITS: u64 = crate::dsl::source::DEFAULT_TRACK_TOTAL_HITS;

/// One sort value of a hit.
#[derive(Clone, Debug, PartialEq)]
pub enum SortValue {
    /// A numeric field's value (or its missing value), or a document id.
    Long(i64),
    /// A keyword's term; `None` for a document without one.
    Term(Option<Vec<u8>>),
    /// The score.
    Score(f32),
    /// A script's number.
    Double(f64),
}

/// One hit: the document and what it is ranked by.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// The global document id.
    pub doc: i32,
    /// The score; `None` when sorted by fields without the score.
    pub score: Option<f32>,
    /// The sort values; empty when sorted by score.
    pub sort: Vec<SortValue>,
}

/// A search's hits on one shard.
#[derive(Clone, Debug, PartialEq)]
pub struct Hits {
    /// How many documents matched, exactly when `exact`.
    pub total: u64,
    /// Whether `total` is exact rather than a lower bound.
    pub exact: bool,
    /// The best score; `None` when sorted by fields or nothing was collected.
    pub max_score: Option<f32>,
    /// The hits after `from`, at most `size`.
    pub hits: Vec<Hit>,
    /// The aggregations' shard-level results, in request order.
    pub aggregations: Vec<Partial>,
    /// Whether the response carries the total (`track_total_hits` not off).
    pub tracked: bool,
    /// What of each hit's `_source` to write.
    pub source: SourceFilter,
    /// The fields each hit's `fields` object carries, in its order.
    pub fields: Vec<HitField>,
    /// Each hit's explanation, in order; empty unless `explain` was asked.
    pub explanations: Vec<Explanation>,
    /// What to highlight in each hit; `None` without `highlight`.
    pub highlight: Option<Plan>,
    /// Which of each hit's `_version`, `_seq_no` and `_primary_term` to write.
    pub meta: HitMeta,
    /// The nested inner hits of the hits, by parent document.
    pub inner: Vec<crate::search_inner_hits::ParentInner>,
    /// Whether `terminate_after` ended the shard's collection; `None` when it
    /// was not asked.
    pub terminated_early: Option<bool>,
}

impl Hits {
    /// Keeps the first `cap` hits and, with them, their explanations; the
    /// total and the aggregations are not touched.
    pub fn truncate(&mut self, cap: usize) {
        self.hits.truncate(cap);
        if self.explanations.len() > cap {
            self.explanations.truncate(cap);
        }
    }
}

pub use crate::search_error::ShardError;

/// The longest term Lucene indexes (`IndexWriter.MAX_TERM_LENGTH`): the
/// most bytes one keyword sort value can hold.
pub const MAX_TERM_BYTES: usize = 32_766;

/// The most bytes a search of `source` on `shard` holds for its hits: the
/// engine's collected hits and this module's, each with its sort values,
/// and the engine's collector queue and value slots, which live beside its
/// collected hits while they are built (counted as one more engine hit),
/// for no more hits than the window or the shard's documents. A keyword
/// sort value is counted at [`MAX_TERM_BYTES`] twice: two copies of a term
/// are alive at once, the engine's slot and hit, then its hit and this
/// module's. What the caller reserves before searching. A full window of
/// 10,000 hits sorted by a long was measured at 157 bytes a hit beyond a
/// one-hit search; this counts 192.
pub fn hits_bytes<S: BlobStore + 'static>(source: &SearchSource, shard: &OpenShard<S>) -> usize {
    let docs = usize::try_from(shard.reader.max_doc()).unwrap_or(0);
    let window = source
        .from
        .saturating_add(source.size)
        .min(MAX_RESULT_WINDOW)
        .min(docs);
    // Each `top_hits` collects its own window, in its own sort.
    let inner = source
        .aggs
        .iter()
        .filter_map(|agg| agg.top_hits.as_deref())
        .fold(0usize, |total, spec| {
            let window = spec
                .from
                .saturating_add(spec.size)
                .min(top_hits::MAX_INNER_RESULT_WINDOW)
                .min(docs);
            total.saturating_add(window_bytes(window, &spec.sort, shard))
        });
    let inner = inner.saturating_add(top_hits_sub::reserved(&source.aggs, shard, docs));
    // A numeric `terms` counts each distinct value its matches hold: at most
    // one per document of the shard. A keyword one counts into the global
    // ordinals of its field, whose size the segments state.
    let counted = source
        .aggs
        .iter()
        .filter_map(|agg| agg.terms.as_deref().map(|spec| (agg, spec)))
        .fold(0usize, |total, (agg, _)| {
            let keyword = shard
                .meta
                .mapping
                .get(&agg.field)
                .is_some_and(|f| f.ty == FieldType::Keyword);
            total.saturating_add(if keyword {
                keyword_subs::reserved(shard, agg, keyword_collect::Parent::TOP)
            } else {
                docs.saturating_mul(TERMS_BYTES_PER_VALUE)
            })
        });
    window_bytes(window, &source.sort, shard)
        .saturating_add(inner)
        .saturating_add(counted)
        .saturating_add(crate::aggs::metric::keyword_cardinality::reserved(
            &source.aggs,
            shard,
        ))
        .saturating_add(fixed_subs::reserved(shard, &source.aggs))
        .saturating_add(keyed_subs::reserved(shard, &source.aggs))
        .saturating_add(composite_collect::reserved_all(shard, &source.aggs))
        .saturating_add(state_bytes(&source.aggs))
        .saturating_add(crate::search_nested_bytes::bytes(source, shard))
}

/// What a `terms` holds for each distinct value it counts while it
/// collects: its key, its count and its index entry.
pub const TERMS_BYTES_PER_VALUE: usize = 64;

/// The bytes `window` hits sorted by `sort` hold: see [`hits_bytes`].
pub(crate) fn window_bytes<S: BlobStore + 'static>(
    window: usize,
    sort: &[SortKey],
    shard: &OpenShard<S>,
) -> usize {
    let terms = sort
        .iter()
        .filter(|key| match key {
            SortKey::Field { field, .. } => shard
                .meta
                .mapping
                .get(field)
                .is_some_and(|f| f.ty == FieldType::Keyword),
            SortKey::Score { .. } | SortKey::Script { .. } => false,
        })
        .count();
    let per_hit = std::mem::size_of::<Hit>()
        .saturating_add(std::mem::size_of::<FieldDoc>().saturating_mul(2))
        .saturating_add(sort.len().saturating_mul(
            std::mem::size_of::<SortValue>().saturating_add(std::mem::size_of::<[i64; 2]>()),
        ))
        .saturating_add(terms.saturating_mul(MAX_TERM_BYTES.saturating_mul(2)));
    window.saturating_mul(per_hit)
}

/// The total a search reports, as `SearchPhaseController.TopDocsStats.getTotalHits`
/// gives it for `track_total_hits` of `limit`: a count past the limit is
/// reported as that many, at least. The engine may
/// count further when it has to visit every match anyway, as for a sort.
pub fn reported(counted: u64, relation: TotalHitsRelation, limit: u64) -> (u64, bool) {
    if counted > limit {
        (limit, false)
    } else {
        (counted, relation == TotalHitsRelation::EqualTo)
    }
}

/// Runs `source` on `shard` of index `index`.
pub fn search<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    index: &str,
    source: &SearchSource,
    analyzer: &Analyzer,
) -> Result<Hits, ShardError> {
    search_with(shard, index, source, analyzer, None)
}

/// As [`search`], with `pinned` analysing the text fields an index pins an
/// analyzer for; without it a query on one is refused as
/// `extension_unavailable`.
pub fn search_with<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    index: &str,
    source: &SearchSource,
    analyzer: &Analyzer,
    pinned: Option<&dyn crate::pinned::PinnedTokens>,
) -> Result<Hits, ShardError> {
    let meta = &shard.meta;
    let mut cx = QueryContext {
        mapping: &meta.mapping,
        analyzer,
        max_doc: shard.reader.max_doc(),
        knn: None,
        pinned,
        now_millis: source.now_millis,
    };
    let (knn, early) = crate::knn::prepare(shard, source.query.as_ref(), &cx)
        .map_err(|f| ShardError::bad_request(shard_failure(f, index, &meta.uuid)))?
        .unzip();
    cx.knn = knn.as_ref();
    let query = match &source.query {
        Some(q) => to_lucene(q, &cx),
        None => to_lucene(&crate::dsl::Query::MatchAll { boost: 1.0 }, &cx),
    }
    .map_err(|f| ShardError::of_query(shard_failure(f, index, &meta.uuid)))?;
    let window = source.from.saturating_add(source.size);
    search_options::check_window(window, meta.max_result_window())?;
    if let Some(what) = source.explain.then(|| explain::refusal(&query)).flatten() {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("explain of {what} is not supported by this node yet"),
        )));
    }
    let query = crate::dsl::queries::nested_build::skip_nested(query, &meta.mapping, knn.is_some())
        .map_err(ShardError::bad_request)?;
    let sort = sort_fields(&source.sort, shard, index, Some(&cx))?;
    crate::search_sort_nested::refuse_after(source)?;
    search_options::check(
        source,
        &query,
        sort.as_ref().map(|(_, kinds)| kinds.as_slice()),
    )?;
    docvalue_fields::check_count(&source.docvalue_fields)?;
    let after = if source.search_after.is_empty() {
        None
    } else {
        let is_date =
            |name: &str| matches!(meta.mapping.get(name).map(|f| &f.ty), Some(FieldType::Date));
        let keys: Option<Vec<AfterKey<'_>>> = sort.as_ref().map(|(fields, _)| {
            fields
                .iter()
                .map(|f| AfterKey::of(f, is_date))
                // ALLOC: one key a sort key, at most MAX_SORT_KEYS.
                .collect()
        });
        Some(field_doc(
            source.from,
            &source.search_after,
            keys.as_deref(),
        )?)
    };
    let fields = scored_fields(&query);
    let owned = shard
        .reader
        .field_norms_by_field(&fields)
        .map_err(ShardError::engine)?;
    // ALLOC: one entry a segment.
    let norms: Vec<Option<&HashMap<String, FieldNorms<'_>>>> = owned.iter().map(Some).collect();
    let mut opened = match early {
        Some(opened) => opened,
        None => shard.reader.open_segments().map_err(ShardError::engine)?,
    };
    let inner_plans = crate::search_inner_hits::plan(source.query.as_ref(), shard, index, &cx)?;
    let top_plans = top_hits::plan(&source.aggs, shard, index)?;
    // The bucket limit holds before any filter is built as a query.
    for agg in &source.aggs {
        crate::aggs::bucket::check_limits(agg)?;
    }
    crate::aggs::bucket::nested_agg::check_placement(&source.aggs)?;
    let filter_plans = filters_collect::plan(&source.aggs, &cx)
        .map_err(|f| ShardError::of_plan(shard_failure(f, index, &meta.uuid)))?;
    let numeric_sort = sort
        .as_ref()
        .is_some_and(|(_, kinds)| kinds.contains(&KeyKind::Numeric))
        || top_plans.iter().any(top_hits::TopHitsPlan::numeric)
        || top_hits_sub::any_numeric_sub(&source.aggs, shard);
    if numeric_sort || has_points(&query) || filter_plans.iter().flatten().any(has_points) {
        opened.open_points().map_err(ShardError::engine)?;
    }
    let segments = opened.as_open_segments();
    let mut aggregations = metric::collect(
        &source.aggs,
        shard,
        &segments,
        &query,
        &filter_plans,
        &norms,
    )?;
    top_hits::fill(
        &mut aggregations,
        &source.aggs,
        &top_plans,
        shard,
        &segments,
        &norms,
        &query,
    )?;
    // A total turned off counts nothing past the hits.
    let limit = source.track_total_hits.unwrap_or(0);
    let narrowed = match &source.post_filter {
        Some(filter) => Some(search_post_filter::narrowed(
            &query,
            to_lucene(filter, &cx)
                .map_err(|f| ShardError::bad_request(shard_failure(f, index, &meta.uuid)))?,
        )),
        None => None,
    };
    let mut top = collect_top(
        &segments,
        shard,
        &norms,
        narrowed.as_ref().unwrap_or(&query),
        sort.as_ref(),
        window,
        source.from,
        limit,
        after.as_ref(),
        TopOptions {
            min_score: source.min_score,
            track_scores: source.track_scores,
            terminate_after: source.terminate_after,
        },
    )?;
    if let Some((_, kinds)) = sort.as_ref().filter(|_| source.track_scores)
        && !kinds.contains(&KeyKind::Score)
        && !top.hits.is_empty()
    {
        // A field sort's hits carry no score; `track_scores` has each one
        // scored, as `TopFieldCollector.populateScores` does.
        let searcher = IndexSearcher::new(&segments, &norms).map_err(ShardError::engine)?;
        let scored =
            explain::explain_hits(&searcher, &query, &top.hits).map_err(ShardError::engine)?;
        for (hit, e) in top.hits.iter_mut().zip(&scored) {
            hit.score = Some(e.value);
        }
    }
    let mut hits = Hits {
        total: top.total,
        exact: top.exact,
        max_score: top.max_score,
        hits: top.hits,
        aggregations,
        tracked: source.track_total_hits.is_some(),
        source: source.source.clone(),
        // ALLOC: empty; resolved below when there are hits.
        fields: Vec::new(),
        // ALLOC: empty; explained below when asked.
        explanations: Vec::new(),
        highlight: None,
        meta: HitMeta {
            version: source.version,
            seq_no: source.seq_no_primary_term,
        },
        // ALLOC: empty; filled below when a nested query asks for inner hits.
        inner: Vec::new(),
        terminated_early: top.terminated,
    };
    hits.inner = crate::search_inner_hits::fill(&inner_plans, shard, &segments, &hits.hits)?;
    // The fetch phase resolves the fields only when it has a hit to fetch.
    if !hits.hits.is_empty() && !source.docvalue_fields.is_empty() {
        hits.fields = docvalue_fields::resolve(&source.docvalue_fields, &meta.mapping)?;
    }
    if let Some(spec) = source.highlight.as_ref().filter(|_| !hits.hits.is_empty()) {
        hits.highlight = highlight::plan(spec, &meta.mapping, &query)?;
    }
    if source.explain && !hits.hits.is_empty() {
        let searcher = IndexSearcher::new(&segments, &norms).map_err(ShardError::engine)?;
        hits.explanations =
            explain::explain_hits(&searcher, &query, &hits.hits).map_err(ShardError::engine)?;
    }
    Ok(hits)
}

/// Whether a query has a point clause, which needs the points open.
fn has_points(query: &BooleanQuery) -> bool {
    fn walk(c: &Clause) -> bool {
        match c {
            // A knn query's answer is fixed documents, not points.
            Clause::Extended(e) => !matches!(**e, ExtendedQuery::DocAndScore(_)),
            Clause::Boolean(b) => has_points(b),
            Clause::Boost(b) => walk(&b.inner),
            Clause::ConstantScore(c) => walk(&c.inner),
            Clause::DisjunctionMax(d) => d.disjuncts.iter().any(walk),
            _ => false,
        }
    }
    query
        .must
        .iter()
        .chain(&query.should)
        .chain(&query.filter)
        .chain(&query.must_not)
        .any(walk)
}

/// The text fields a query scores, whose norms it reads.
pub(crate) fn scored_fields(query: &BooleanQuery) -> Vec<String> {
    fn walk(c: &Clause, out: &mut Vec<String>) {
        match c {
            Clause::Term(lucene_search::TermQuery { field, .. })
            | Clause::Fuzzy(lucene_search::FuzzyQuery { field, .. })
            | Clause::Phrase(lucene_search::PhraseQuery { field, .. })
                if !out.contains(field) =>
            {
                // ALLOC: one name a scored field, at most the query's terms.
                out.push(field.clone());
            }
            Clause::Boolean(b) => each(b, out),
            Clause::Boost(b) => walk(&b.inner, out),
            Clause::Extended(e) => {
                match &**e {
                    ExtendedQuery::FunctionScore(q) => walk(q.wrapped_query(), out),
                    // The child query's fields score the parents.
                    ExtendedQuery::ToParentBlockJoin(q) => walk(&q.child, out),
                    _ => {}
                }
            }
            Clause::DisjunctionMax(d) => {
                for c in &d.disjuncts {
                    walk(c, out);
                }
            }
            _ => {}
        }
    }
    fn each(q: &BooleanQuery, out: &mut Vec<String>) {
        for c in q
            .must
            .iter()
            .chain(&q.should)
            .chain(&q.filter)
            .chain(&q.must_not)
        {
            walk(c, out);
        }
    }
    // ALLOC: one entry a scored field.
    let mut out = Vec::new();
    each(query, &mut out);
    out
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
