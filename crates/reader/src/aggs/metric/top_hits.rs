//! The `top_hits` aggregation on one shard: `TopHitsAggregationBuilder.parse`,
//! `TopHitsAggregatorFactory` and `TopHitsAggregator`
//! (server/src/main/java/org/opensearch/search/aggregations/metrics/), at
//! OpenSearch 3.10.0.
//!
//! It answers the top documents of the request's query in its own sort, `from`
//! and `size`, whatever the request's own sort, paging and total are: the
//! matches are counted exactly, and a score sort reports the best score of
//! the window before `from` is skipped. Each hit is fetched as a hit of the
//! response is, from the same `_source`, `docvalue_fields`, `explain` and
//! `highlight` options. The result is the shard-level hits an
//! `InternalTopHits` holds; reducing several shards' is the coordinator's,
//! which has to fetch the hits it keeps.

use std::collections::HashMap;

use lucene_search::field_norms::FieldNorms;
use lucene_search::index_searcher::IndexSearcher;
use lucene_search::multi_segment::OpenSegment;
use lucene_search::{BooleanQuery, Explanation};
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::metric::{MetricAgg, Partial, Value};
use crate::dsl::SortKey;
use crate::dsl::error::DslError;
use crate::dsl::sort::parse_sort;
use crate::dsl::source::unknown_key;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, int_value};
use crate::failure::Failure;
use crate::fetch::docvalue_fields::{self, DocValueField, HitField, parse_docvalue_fields};
use crate::fetch::explain;
use crate::fetch::highlight::{self, Plan as HighlightPlan};
use crate::fetch::highlight_parse::{HighlightSpec, parse_highlight};
use crate::fetch::source_filter::{SourceFilter, parse_source};
use crate::search::{Hit, ShardError};
use crate::search_sort::{Sort, sort_fields};
use crate::search_top::{TopOptions, collect_top};

/// `TopHitsAggregationBuilder.DEFAULT_SIZE`.
pub const DEFAULT_SIZE: usize = 3;

/// `index.max_inner_result_window`'s default: the most `from + size` of one
/// `top_hits` may reach.
pub const MAX_INNER_RESULT_WINDOW: usize = 100;

/// What a `top_hits` aggregation asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct TopHitsSpec {
    /// How many hits to skip.
    pub from: usize,
    /// How many hits to return.
    pub size: usize,
    /// The sort; by score when empty.
    pub sort: Vec<SortKey>,
    /// `_source`: whether and which of each hit's source to return.
    pub source: SourceFilter,
    /// `docvalue_fields`: the fields whose doc values each hit carries.
    pub docvalue_fields: Vec<DocValueField>,
    /// `explain`: whether each hit carries the explanation of its score.
    pub explain: bool,
    /// `highlight`: the fields whose matches each hit marks.
    pub highlight: Option<HighlightSpec>,
}

impl Default for TopHitsSpec {
    fn default() -> Self {
        Self {
            from: 0,
            size: DEFAULT_SIZE,
            // ALLOC: empty; a sort pushes at most MAX_SORT_KEYS.
            sort: Vec::new(),
            source: SourceFilter::default(),
            // ALLOC: empty; parsing pushes one an entry.
            docvalue_fields: Vec::new(),
            explain: false,
            highlight: None,
        }
    }
}

/// One shard's `top_hits` result: the hits and what each is written with.
#[derive(Clone, Debug, PartialEq)]
pub struct TopHitsResult {
    /// How many documents matched, always exactly.
    pub total: u64,
    /// The best score of the window; `None` when sorted by fields.
    pub max_score: Option<f32>,
    /// The hits ahead of `from`, which the shard does not write but a
    /// coordinator's merge ranks: one of them can be in the merged page.
    pub before: Vec<Hit>,
    /// The hits after `from`.
    pub hits: Vec<Hit>,
    /// What of each hit's `_source` to write.
    pub source: SourceFilter,
    /// The fields each hit's `fields` object carries, in its order.
    pub fields: Vec<HitField>,
    /// Each hit's explanation, in order; empty unless `explain` was asked.
    pub explanations: Vec<Explanation>,
    /// The explanations of `before`, in order, so a hit a merge pulls from
    /// there is explained like the rest; empty unless `explain` was asked.
    pub before_explanations: Vec<Explanation>,
    /// What to highlight in each hit; `None` without `highlight`.
    pub highlight: Option<HighlightPlan>,
    /// The shard this result came from among a search's shards, for the
    /// coordinator that merges it; zero until tagged.
    pub origin: usize,
}

impl TopHitsResult {
    /// The result of no match: what the aggregation holds before the shard
    /// has searched, and what it answers for no document.
    pub fn empty() -> Self {
        Self {
            total: 0,
            max_score: None,
            // ALLOC: empty.
            before: Vec::new(),
            // ALLOC: empty.
            hits: Vec::new(),
            source: SourceFilter::default(),
            // ALLOC: empty.
            fields: Vec::new(),
            // ALLOC: empty.
            explanations: Vec::new(),
            // ALLOC: empty.
            before_explanations: Vec::new(),
            highlight: None,
            origin: 0,
        }
    }

    /// What the response writes the hits from.
    pub(crate) fn view(&self) -> crate::response::HitView<'_> {
        crate::response::HitView {
            hits: &self.hits,
            source: &self.source,
            fields: &self.fields,
            explanations: &self.explanations,
            matched: &[],
            named_scores: false,
            highlight: self.highlight.as_ref(),
            meta: crate::fetch::hit_meta::HitMeta::default(),
            inner: &[],
        }
    }
}

/// A key of `top_hits` OpenSearch reads that this node does not serve yet.
fn unsupported(key: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::unsupported(&format!("the [top_hits] aggregation's [{key}]"))
}

/// `from` or `size`: `TopHitsAggregationBuilder.from` and `size` refuse a
/// negative number.
fn non_negative(key: &str, n: i32, name: &str) -> Result<usize, DslError> {
    usize::try_from(n).map_err(|_| {
        DslError::illegal(
            // ALLOC: error path; one message.
            format!("[{key}] must be greater than or equal to 0. Found [{n}] in [{name}]"),
        )
    })
}

/// `TopHitsAggregationBuilder.parse` over the aggregation's object, the
/// parser on its start: its options, none of them required.
pub fn parse_top_hits_body(p: &mut Parser<'_>, name: &str) -> Result<TopHitsSpec, DslError> {
    let mut spec = TopHitsSpec::default();
    let mut key = "";
    while let Some(token) = p.next_token() {
        match token {
            Kind::EndObject => break,
            Kind::FieldName => key = p.text(),
            // The parser's value branch takes no null.
            Kind::Null => return Err(unknown_key(p, Kind::Null, key)),
            kind if kind.is_value() => match key {
                "from" => spec.from = non_negative("from", int_value(p)?, name)?,
                "size" => spec.size = non_negative("size", int_value(p)?, name)?,
                "explain" => spec.explain = bool_value(p)?,
                "track_scores" | "version" | "seq_no_primary_term" => {
                    if bool_value(p)? {
                        return Err(unsupported(key));
                    }
                }
                "stored_fields" => return Err(unsupported(key)),
                "_source" => spec.source = parse_source(&mut p.clone(), key)?,
                "sort" => spec.sort = parse_sort(&mut p.clone())?,
                _ => return Err(unknown_key(p, kind, key)),
            },
            Kind::StartObject => match key {
                "sort" => spec.sort = parse_sort(p)?,
                "_source" => spec.source = parse_source(p, key)?,
                "highlight" => spec.highlight = Some(parse_highlight(p)?),
                "script_fields" => return Err(unsupported(key)),
                _ => return Err(unknown_key(p, Kind::StartObject, key)),
            },
            Kind::StartArray => match key {
                "sort" => spec.sort = parse_sort(p)?,
                "_source" => spec.source = parse_source(p, key)?,
                "docvalue_fields" => spec.docvalue_fields = parse_docvalue_fields(p)?,
                "stored_fields" | "fields" => return Err(unsupported(key)),
                _ => return Err(unknown_key(p, Kind::StartArray, key)),
            },
            kind => return Err(unknown_key(p, kind, key)),
        }
    }
    Ok(spec)
}

/// What a shard checks and resolves of a `top_hits` before it searches.
pub(crate) struct TopHitsPlan {
    pub(super) sort: Sort,
    pub(super) window: usize,
}

impl TopHitsPlan {
    /// Whether the sort reads a numeric field, which needs the points open.
    pub(crate) fn numeric(&self) -> bool {
        self.sort
            .as_ref()
            .is_some_and(|(_, kinds)| crate::search_sort::reads_points(kinds))
    }
}

/// `TopHitsAggregatorFactory` and `TopHitsAggregator`'s checks, one plan
/// for each `top_hits` of `aggs` in request order: the result window, a
/// size of zero (which Lucene's collectors refuse), the sort and the
/// `docvalue_fields` count.
pub(crate) fn plan<S: BlobStore + 'static>(
    aggs: &[MetricAgg],
    shard: &OpenShard<S>,
    index: &str,
) -> Result<Vec<TopHitsPlan>, ShardError> {
    // ALLOC: one plan a `top_hits`, bounded by the request's aggregations.
    let mut plans = Vec::new();
    for (agg, spec) in aggs
        .iter()
        .filter_map(|a| a.top_hits.as_deref().map(|s| (a, s)))
    {
        let window = spec.from.saturating_add(spec.size);
        if window > MAX_INNER_RESULT_WINDOW {
            return Err(ShardError::bad_request(Failure::plain(
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!(
                    "Top hits result window is too large, the top hits aggregator [{}]'s from + \
                     size must be less than or equal to: [{MAX_INNER_RESULT_WINDOW}] but was \
                     [{window}]. This limit can be set by changing the \
                     [index.max_inner_result_window] index level setting.",
                    agg.name
                ),
            )));
        }
        docvalue_fields::check_count(&spec.docvalue_fields)?;
        crate::search_sort_nested::refuse_script(&spec.sort, "in top_hits")?;
        let sort = sort_fields(&spec.sort, shard, index, None)?;
        if spec.size == 0 {
            return Err(ShardError::bad_request(Failure::plain(
                "illegal_argument_exception",
                "numHits must be > 0; please use TotalHitCountCollectorManager if you just \
                 need the total hit count",
            )));
        }
        plans.push(TopHitsPlan { sort, window });
    }
    Ok(plans)
}

/// The matches counted exactly: `TopDocsCollector`'s threshold of
/// `Integer.MAX_VALUE`.
const EXACT: u64 = i32::MAX as u64;

/// Runs each `top_hits` of `aggs` over `query` and puts its result in its
/// place among `parts` (which `collect` filled in request order).
#[allow(clippy::too_many_arguments)]
pub(crate) fn fill<S: BlobStore + 'static>(
    parts: &mut [Partial],
    aggs: &[MetricAgg],
    plans: &[TopHitsPlan],
    shard: &OpenShard<S>,
    segments: &[OpenSegment<'_>],
    norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
    query: &BooleanQuery,
) -> Result<(), ShardError> {
    let mut plans = plans.iter();
    for (agg, part) in aggs.iter().zip(parts.iter_mut()) {
        let Some(spec) = agg.top_hits.as_deref() else {
            continue;
        };
        let Some(plan) = plans.next() else { continue };
        let mut top = collect_top(
            segments,
            shard,
            norms,
            query,
            plan.sort.as_ref(),
            plan.window,
            0,
            EXACT,
            None,
            TopOptions::default(),
        )?;
        if plan.sort.is_some() {
            // Scores are tracked only on request (`track_scores`, not served):
            // a sort by the score still writes its value as a sort value.
            top.hits.iter_mut().for_each(|h| h.score = None);
        }
        let mut before = top.hits;
        let hits = before.split_off(spec.from.min(before.len()));
        let mut result = TopHitsResult {
            total: top.total,
            max_score: top.max_score,
            before,
            hits,
            source: spec.source.clone(),
            // ALLOC: empty; resolved below when there are hits.
            fields: Vec::new(),
            // ALLOC: empty; explained below when asked.
            explanations: Vec::new(),
            // ALLOC: empty; explained below when asked.
            before_explanations: Vec::new(),
            highlight: None,
            origin: 0,
        };
        if spec.explain
            && let Some(what) = explain::refusal(query)
        {
            return Err(ShardError::bad_request(Failure::plain(
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!("explain of {what} is not supported by this node yet"),
            )));
        }
        // The fetch phase resolves them only when it has a hit to fetch. With
        // only hits ahead of `from` a coordinator's merge may still pull one,
        // so they are resolved if they can be, and an error is left to the
        // shard whose hits are written.
        let fetches = !result.hits.is_empty();
        if fetches || !result.before.is_empty() {
            let fields = if spec.docvalue_fields.is_empty() {
                // ALLOC: empty.
                Ok(Vec::new())
            } else {
                docvalue_fields::resolve(&spec.docvalue_fields, &shard.meta.mapping)
            };
            let plan = match &spec.highlight {
                Some(h) => highlight::plan(h, &shard.meta.mapping, query),
                None => Ok(None),
            };
            match (fields, plan) {
                (Ok(fields), Ok(plan)) => {
                    result.fields = fields;
                    result.highlight = plan;
                }
                (Err(e), _) | (_, Err(e)) if fetches => return Err(e),
                _ => {}
            }
            if spec.explain {
                let searcher = IndexSearcher::new(segments, norms).map_err(ShardError::engine)?;
                result.explanations = explain::explain_hits(&searcher, query, &result.hits)
                    .map_err(ShardError::engine)?;
                result.before_explanations =
                    explain::explain_hits(&searcher, query, &result.before)
                        .map_err(ShardError::engine)?;
            }
        }
        // ALLOC: one box a `top_hits`, bounded by the request's.
        part.value = Value::TopHits(Box::new(result));
    }
    Ok(())
}
