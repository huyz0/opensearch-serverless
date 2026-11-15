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
use lucene_search::top_field::{FieldDoc, SortField, SortType, search_sorted};
use lucene_search::{BooleanQuery, Clause};
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::dsl::{SearchSource, SortKey};
use crate::failure::{Failure, Meta};
use crate::lucene_query::{QueryContext, shard_failure, to_lucene};
use crate::mapping::FieldType;

/// `index.max_result_window`'s default: the most `from + size` may reach.
pub const MAX_RESULT_WINDOW: usize = 10_000;

/// `track_total_hits`' default: matches are counted exactly up to here.
pub const TRACK_TOTAL_HITS: u64 = 10_000;

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
    fn bad_request(failure: Failure) -> Self {
        Self {
            status: 400,
            failure,
        }
    }

    fn engine(e: impl std::fmt::Display) -> Self {
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
/// for no more hits than the window or the shard's documents. A keyword
/// sort value is counted at [`MAX_TERM_BYTES`] twice, as the engine's copy
/// and this module's. What the caller reserves before searching.
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
        .saturating_add(std::mem::size_of::<FieldDoc>())
        .saturating_add(source.sort.len().saturating_mul(
            std::mem::size_of::<SortValue>().saturating_add(std::mem::size_of::<[i64; 2]>()),
        ))
        .saturating_add(terms.saturating_mul(MAX_TERM_BYTES.saturating_mul(2)));
    window.saturating_mul(per_hit)
}

/// How a key orders, and how its values read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyKind {
    Score,
    Numeric,
    Term,
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
    let sort = sort_fields(&source.sort, shard, index)?;
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
    match sort {
        Some((keys, kinds)) if window > 0 => {
            let top = search_sorted(
                &segments,
                shard.reader.segment_readers(),
                &query,
                &norms,
                &keys,
                window,
                TRACK_TOTAL_HITS,
                None,
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
            Ok(Hits {
                total: top.total.value,
                exact: top.total.relation == TotalHitsRelation::EqualTo,
                max_score: None,
                hits,
            })
        }
        _ => {
            let searcher = IndexSearcher::new(&segments, &norms).map_err(ShardError::engine)?;
            let mut collector =
                TopDocsCollector::with_total_hits_threshold(window, TRACK_TOTAL_HITS);
            searcher
                .search_collector(&query, &mut collector)
                .map_err(ShardError::engine)?;
            let top = collector.top_docs();
            let total = collector.total_hits();
            Ok(Hits {
                total: total.value,
                exact: total.relation == TotalHitsRelation::EqualTo,
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

/// Whether a query has a point clause, which needs the points open.
fn has_points(query: &BooleanQuery) -> bool {
    fn walk(c: &Clause) -> bool {
        match c {
            Clause::Extended(_) => true,
            Clause::Boolean(b) => has_points(b),
            Clause::Boost(b) => walk(&b.inner),
            Clause::ConstantScore(c) => walk(&c.inner),
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
            Clause::Term(t) if !out.contains(&t.field) => {
                // ALLOC: one name a scored field, at most the query's terms.
                out.push(t.field.clone());
            }
            Clause::Boolean(b) => each(b, out),
            Clause::Boost(b) => walk(&b.inner, out),
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

type Sort = Option<(Vec<SortField>, Vec<KeyKind>)>;

/// `SortBuilder.buildSort`: `None` for a sort by score alone.
fn sort_fields<S: BlobStore + 'static>(
    keys: &[SortKey],
    shard: &OpenShard<S>,
    index: &str,
) -> Result<Sort, ShardError> {
    if keys.is_empty() || keys == [SortKey::Score { ascending: false }] {
        return Ok(None);
    }
    // ALLOC: one entry a sort key, at most MAX_SORT_KEYS.
    let mut fields = Vec::with_capacity(keys.len());
    // ALLOC: one entry a sort key, at most MAX_SORT_KEYS.
    let mut kinds = Vec::with_capacity(keys.len());
    for key in keys {
        let (field, kind) = match key {
            SortKey::Score { ascending } => {
                let mut f = SortField::score();
                f.reverse = *ascending;
                (f, KeyKind::Score)
            }
            SortKey::Field { field, ascending } => field_sort(field, !*ascending, shard, index)?,
        };
        fields.push(field);
        kinds.push(kind);
    }
    Ok(Some((fields, kinds)))
}

/// `FieldSortBuilder.build`: missing values last, whichever the order.
fn field_sort<S: BlobStore + 'static>(
    name: &str,
    reverse: bool,
    shard: &OpenShard<S>,
    index: &str,
) -> Result<(SortField, KeyKind), ShardError> {
    if name == "_doc" {
        let mut f = SortField::doc();
        f.reverse = reverse;
        return Ok((f, KeyKind::Numeric));
    }
    let Some(field) = shard.meta.mapping.get(name) else {
        return Err(ShardError::bad_request(
            Failure::opensearch(
                "query_shard_exception",
                // ALLOC: error path; one message.
                format!("No mapping found for [{name}] in order to sort on"),
            )
            // ALLOC: error path; the index name.
            .with_meta("index", Meta::Str(index.to_owned()))
            // ALLOC: error path; the index uuid.
            .with_meta("index_uuid", Meta::Str(shard.meta.uuid.clone())),
        ));
    };
    let unsupported = |what: &str| {
        ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("sorting on {what} is not supported by this node yet"),
        ))
    };
    if matches!(field.ty, FieldType::Text) {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "Text fields are not optimised for operations that require per-document field \
                 data like aggregations and sorting, so these operations are disabled by default. \
                 Please use a keyword field instead. Alternatively, set fielddata=true on [{name}] \
                 in order to load field data by uninverting the inverted index. Note that this can \
                 use significant memory."
            ),
        )));
    }
    if !field.doc_values {
        // ALLOC: error path; one message.
        return Err(unsupported(&format!(
            "the field [{name}] without doc values"
        )));
    }
    let (ty, min, max) = match &field.ty {
        FieldType::Keyword => {
            let mut f = SortField::string(name, reverse);
            f.missing = i64::from(!reverse);
            return Ok((f, KeyKind::Term));
        }
        FieldType::Int { .. } => (SortType::Int, i64::from(i32::MIN), i64::from(i32::MAX)),
        FieldType::Long | FieldType::Date => (SortType::Long, i64::MIN, i64::MAX),
        // ALLOC: error path; one message.
        FieldType::Unsupported(what) => return Err(unsupported(&format!("{what} [{name}]"))),
        FieldType::Text => return Err(unsupported("a text field")),
    };
    let mut f = SortField::numeric(name, ty, reverse);
    f.missing = if reverse { min } else { max };
    Ok((f, KeyKind::Numeric))
}
