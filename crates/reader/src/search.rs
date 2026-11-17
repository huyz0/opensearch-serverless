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
use lucene_search::collector::{TopDocsCollector, TotalHitsRelation};
use lucene_search::field_norms::FieldNorms;
use lucene_search::index_searcher::IndexSearcher;
use lucene_search::top_field::{FieldDoc, search_sorted};
use lucene_search::{BooleanQuery, Clause, Explanation};
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::metric::{self, Partial, state_bytes};
use crate::dsl::search_after::{AfterKey, field_doc};
use crate::dsl::{SearchSource, SortKey};
use crate::failure::Failure;
use crate::fetch::docvalue_fields::{self, HitField};
use crate::fetch::explain;
use crate::fetch::highlight::{self, Plan};
use crate::fetch::source_filter::SourceFilter;
use crate::lucene_query::{QueryContext, shard_failure, to_lucene};
use crate::mapping::FieldType;
use crate::search_sort::{KeyKind, sort_fields};

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
}

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

    pub(crate) fn engine(e: impl std::fmt::Display) -> Self {
        Self {
            status: 500,
            // ALLOC: error path; one message.
            failure: Failure::opensearch("exception", e.to_string()),
        }
    }
}

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
    let terms = source
        .sort
        .iter()
        .filter(|key| match key {
            SortKey::Field { field, .. } => shard
                .meta
                .mapping
                .get(field)
                .is_some_and(|f| f.ty == FieldType::Keyword),
            SortKey::Score { .. } => false,
        })
        .count();
    let per_hit = std::mem::size_of::<Hit>()
        .saturating_add(std::mem::size_of::<FieldDoc>().saturating_mul(2))
        .saturating_add(source.sort.len().saturating_mul(
            std::mem::size_of::<SortValue>().saturating_add(std::mem::size_of::<[i64; 2]>()),
        ))
        .saturating_add(terms.saturating_mul(MAX_TERM_BYTES.saturating_mul(2)));
    window
        .saturating_mul(per_hit)
        .saturating_add(state_bytes(&source.aggs))
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
    let meta = &shard.meta;
    let cx = QueryContext {
        mapping: &meta.mapping,
        analyzer,
        max_doc: shard.reader.max_doc(),
    };
    let query = match &source.query {
        Some(q) => to_lucene(q, &cx),
        None => to_lucene(&crate::dsl::Query::MatchAll { boost: 1.0 }, &cx),
    }
    .map_err(|f| ShardError::bad_request(shard_failure(f, index, &meta.uuid)))?;
    let window = source.from.saturating_add(source.size);
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
    if let Some(what) = source.explain.then(|| explain::refusal(&query)).flatten() {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("explain of {what} is not supported by this node yet"),
        )));
    }
    let sort = sort_fields(&source.sort, shard, index)?;
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
    let mut opened = shard.reader.open_segments().map_err(ShardError::engine)?;
    let numeric_sort = sort
        .as_ref()
        .is_some_and(|(_, kinds)| kinds.contains(&KeyKind::Numeric));
    if numeric_sort || has_points(&query) {
        opened.open_points().map_err(ShardError::engine)?;
    }
    let segments = opened.as_open_segments();
    let aggregations = metric::collect(&source.aggs, shard, &segments, &query)?;
    // A total turned off counts nothing past the hits.
    let limit = source.track_total_hits.unwrap_or(0);
    let mut hits = match sort {
        Some((keys, kinds)) if window > 0 => {
            let top = search_sorted(
                &segments,
                shard.reader.segment_readers(),
                &query,
                &norms,
                &keys,
                window,
                limit,
                after.as_ref(),
            )
            .map_err(ShardError::engine)?;
            let score_at = kinds.iter().position(|k| *k == KeyKind::Score);
            let hits = top
                .hits
                .iter()
                .skip(source.from)
                .map(|h| sorted_hit(h, &kinds, score_at))
                // ALLOC: one hit a result, at most MAX_RESULT_WINDOW.
                .collect();
            let (total, exact) = reported(top.total.value, top.total.relation, limit);
            Ok(Hits {
                total,
                exact,
                max_score: None,
                hits,
                aggregations,
                tracked: source.track_total_hits.is_some(),
                source: source.source.clone(),
                // ALLOC: empty; resolved below when there are hits.
                fields: Vec::new(),
                // ALLOC: empty; explained below when asked.
                explanations: Vec::new(),
                highlight: None,
            })
        }
        _ => {
            let searcher = IndexSearcher::new(&segments, &norms).map_err(ShardError::engine)?;
            let mut collector = TopDocsCollector::with_total_hits_threshold(window, limit);
            searcher
                .search_collector(&query, &mut collector)
                .map_err(ShardError::engine)?;
            let top = collector.top_docs();
            let counted = collector.total_hits();
            let (total, exact) = reported(counted.value, counted.relation, limit);
            Ok(Hits {
                total,
                exact,
                max_score: top.first().map(|h| h.score),
                hits: top
                    .iter()
                    .skip(source.from)
                    .map(|h| Hit {
                        doc: h.doc_id,
                        score: Some(h.score),
                        // ALLOC: empty.
                        sort: Vec::new(),
                    })
                    // ALLOC: one hit a result, at most MAX_RESULT_WINDOW.
                    .collect(),
                aggregations,
                tracked: source.track_total_hits.is_some(),
                source: source.source.clone(),
                // ALLOC: as above.
                fields: Vec::new(),
                // ALLOC: as above.
                explanations: Vec::new(),
                highlight: None,
            })
        }
    }?;
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

/// Whether a query has a point clause, which needs the points open.
fn has_points(query: &BooleanQuery) -> bool {
    fn walk(c: &Clause) -> bool {
        match c {
            Clause::Extended(_) => true,
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
fn scored_fields(query: &BooleanQuery) -> Vec<String> {
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
