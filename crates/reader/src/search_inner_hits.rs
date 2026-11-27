//! `inner_hits` of a `nested` query on one shard: `NestedQueryBuilder`'s inner
//! hit context and `InnerHitsPhase` (server/src/main/java/org/opensearch/search/fetch/subphase/InnerHitsPhase.java)
//! at OpenSearch 3.10.0. Each parent hit gets the children its nested query
//! matched, found by a `ParentChildrenBlockJoinQuery` over the parent's own
//! block, so a parent's work is bounded by its block and `from + size`.

use std::collections::HashMap;
use std::sync::Arc;

use lucene_search::field_norms::FieldNorms;
use lucene_search::join::{ParentChildrenBlockJoinQuery, QueryBitSetProducer};
use lucene_search::multi_segment::OpenSegment;
use lucene_search::{BooleanQuery, Clause};
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::dsl::Query;
use crate::dsl::queries::More;
use crate::dsl::queries::nested::NestedQuery;
use crate::dsl::queries::nested_build::type_filter;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, build};
use crate::search::{Hit, ShardError};
use crate::search_sort::{Sort, sort_fields};
use crate::search_top::{TopOptions, collect_top};

/// `index.max_inner_result_window`'s default.
pub const MAX_INNER_RESULT_WINDOW: usize = 100;

/// The matches counted exactly.
const EXACT: u64 = i32::MAX as u64;

/// What a shard checks and builds of one `inner_hits` before it searches.
pub(crate) struct InnerPlan {
    name: String,
    path: String,
    from: usize,
    size: usize,
    source_skip: bool,
    sort: Sort,
    child: Clause,
    parents: Arc<QueryBitSetProducer>,
    /// The nested query's own query, kept when it names queries: the names a child lists.
    named: Option<Query>,
}

/// One `inner_hits` of one parent hit.
#[derive(Clone, Debug, PartialEq)]
pub struct InnerResult {
    /// The name the hits are reported under.
    pub name: String,
    /// The nested path the hits are children of.
    pub path: String,
    /// How many children matched.
    pub total: u64,
    /// The best score; `None` when sorted by fields.
    pub max_score: Option<f32>,
    /// The children after `from`, at most `size`.
    pub hits: Vec<Hit>,
    /// Each child's position among its parent's children of the path.
    pub offsets: Vec<usize>,
    /// Whether the children carry no `_source`.
    pub source_skip: bool,
    /// The names of the queries inside the nested query that each child
    /// matches, in order (the score is `NaN`, as OpenSearch leaves it for an
    /// inner hit); empty when the nested query's own query names none.
    pub matched: Vec<Vec<(String, f32)>>,
}

/// The inner hits of one parent hit, by its document.
#[derive(Clone, Debug, PartialEq)]
pub struct ParentInner {
    /// The parent's global document id.
    pub doc: i32,
    /// One result for each `inner_hits`, in request order.
    pub results: Vec<InnerResult>,
}

fn refuse(reason: &str) -> ShardError {
    ShardError::bad_request(Failure::plain("illegal_argument_exception", reason))
}

/// The nested queries of `q` that carry `inner_hits` and whose parents the
/// search returns: through `bool`'s `must`, `should` and `filter`,
/// `constant_score`, `dis_max` and `function_score`'s query.
fn find<'a>(q: &'a Query, out: &mut Vec<&'a NestedQuery>) -> Result<(), ShardError> {
    match q {
        Query::Named(n) => find(&n.query, out)?,
        Query::Bool(b) => {
            for c in b.must.iter().chain(&b.should).chain(&b.filter) {
                find(c, out)?;
            }
        }
        Query::More(m) => match &**m {
            More::Nested(n, _) => {
                if n.inner_hits.is_some() {
                    // ALLOC: one entry an `inner_hits`, bounded by the request body.
                    out.push(n);
                }
                let mut inside = Vec::new();
                find(&n.query, &mut inside)?;
                if !inside.is_empty() {
                    return Err(refuse(
                        "[inner_hits] of a nested query inside another nested query is not \
                         supported by this node yet",
                    ));
                }
            }
            More::ConstantScore { filter, .. } => find(filter, out)?,
            More::DisMax { queries, .. } => {
                for c in queries {
                    find(c, out)?;
                }
            }
            More::FunctionScore(f, _) => {
                if let Some(c) = &f.query {
                    find(c, out)?;
                }
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

/// The plans of the `inner_hits` of `query`, none when it has none.
///
/// # Errors
/// A window over `index.max_inner_result_window`, a repeated name, an
/// object that is not nested or is inside another, or a numeric sort.
pub(crate) fn plan<S: BlobStore + 'static>(
    query: Option<&Query>,
    shard: &OpenShard<S>,
    index: &str,
    cx: &QueryContext<'_>,
) -> Result<Vec<InnerPlan>, ShardError> {
    // ALLOC: one entry an `inner_hits`, bounded by the request body.
    let mut found = Vec::new();
    if let Some(q) = query {
        find(q, &mut found)?;
    }
    // ALLOC: one plan an `inner_hits`, bounded by the request body.
    let mut plans: Vec<InnerPlan> = Vec::new();
    for n in found {
        let Some(spec) = &n.inner_hits else { continue };
        let name = spec.name.as_deref().unwrap_or(&n.path);
        if plans.iter().any(|p| p.name == name) {
            // ALLOC: error path; one message.
            return Err(refuse(&format!(
                // ALLOC: error path; one message.
                "[inner_hits] already contains an entry for key [{name}]"
            )));
        }
        let window = spec.from.saturating_add(spec.size);
        if window > MAX_INNER_RESULT_WINDOW {
            // ALLOC: error path; one message.
            return Err(refuse(&format!(
                // ALLOC: error path; one message.
                "Inner result window is too large, the inner hit definition's [{name}]'s from + \
                 size must be less than or equal to: [{MAX_INNER_RESULT_WINDOW}] but was \
                 [{window}]. This limit can be set by changing the \
                 [index.max_inner_result_window] index level setting."
            )));
        }
        let mapping = &shard.meta.mapping;
        if !mapping.is_nested(&n.path) {
            if n.ignore_unmapped {
                continue;
            }
            // ALLOC: error path; one message.
            return Err(refuse(&format!(
                // ALLOC: error path; one message.
                "[nested] failed to find nested object under path [{}]",
                n.path
            )));
        }
        if mapping.nested_parent(&n.path).is_some() {
            return Err(refuse(
                "[inner_hits] of a nested object inside another is not supported by this node yet",
            ));
        }
        crate::search_sort_nested::refuse_script(&spec.sort, "in inner hits")?;
        let sort = sort_fields(&spec.sort, shard, index, Some(cx))?;
        if sort
            .as_ref()
            .is_some_and(|(_, kinds)| crate::search_sort::reads_points(kinds))
        {
            return Err(refuse(
                "[inner_hits] [sort] on a numeric field is not supported by this node yet",
            ));
        }
        let inner = build(&n.query, cx).map_err(|f| {
            ShardError::of_query(crate::lucene_query::shard_failure(
                f,
                index,
                &shard.meta.uuid,
            ))
        })?;
        let child = BooleanQuery {
            // ALLOC: two clauses.
            must: vec![inner],
            filter: vec![type_filter(&n.path)],
            ..BooleanQuery::default()
        };
        plans.push(InnerPlan {
            // ALLOC: one name an `inner_hits`.
            name: name.to_owned(),
            // ALLOC: one path an `inner_hits`.
            path: n.path.clone(),
            from: spec.from,
            size: spec.size,
            source_skip: spec.source_skip,
            sort,
            // ALLOC: one box an `inner_hits`.
            child: Clause::Boolean(Box::new(child)),
            parents: Arc::new(QueryBitSetProducer::new(
                crate::dsl::queries::nested_build::non_nested(),
            )),
            // ALLOC: one copy of a query that names queries, bounded by the request body.
            named: (!crate::search_named::collect(&n.query).is_empty()).then(|| n.query.clone()),
        });
    }
    Ok(plans)
}

/// The text fields the inner queries score, whose norms they read.
fn norm_fields(plans: &[InnerPlan]) -> Vec<String> {
    // ALLOC: one name a field the inner queries score.
    let mut out = Vec::new();
    for p in plans {
        if let Clause::Boolean(b) = &p.child {
            out.extend(crate::search::scored_fields(b));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The inner hits of `hits` over `segments`; none without a plan or a hit.
pub(crate) fn fill<S: BlobStore + 'static>(
    plans: &[InnerPlan],
    shard: &OpenShard<S>,
    segments: &[OpenSegment<'_>],
    hits: &[Hit],
    cx: &QueryContext<'_>,
    index: &str,
) -> Result<Vec<ParentInner>, ShardError> {
    if plans.is_empty() || hits.is_empty() {
        // ALLOC: empty.
        return Ok(Vec::new());
    }
    let owned = shard
        .reader
        .field_norms_by_field(&norm_fields(plans))
        .map_err(ShardError::engine)?;
    // ALLOC: one entry a segment.
    let norms: Vec<_> = owned.iter().map(Some).collect();
    collect(plans, shard, segments, &norms, hits, (cx, index))
}

/// The inner hits of each of `hits`, over `segments`.
fn collect<S: BlobStore + 'static>(
    plans: &[InnerPlan],
    shard: &OpenShard<S>,
    segments: &[OpenSegment<'_>],
    norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
    hits: &[Hit],
    (cx, index): (&QueryContext<'_>, &str),
) -> Result<Vec<ParentInner>, ShardError> {
    // ALLOC: one entry a hit, bounded by the request's window.
    let mut out = Vec::with_capacity(hits.len());
    for hit in hits {
        // ALLOC: one result an `inner_hits`, bounded by the request body.
        let mut results = Vec::with_capacity(plans.len());
        for plan in plans {
            let query = BooleanQuery {
                // ALLOC: one clause.
                must: vec![
                    ParentChildrenBlockJoinQuery::new(
                        Arc::clone(&plan.parents) as _,
                        plan.child.clone(),
                        hit.doc,
                    )
                    .into(),
                ],
                ..BooleanQuery::default()
            };
            let mut top = collect_top(
                segments,
                shard,
                norms,
                &query,
                plan.sort.as_ref(),
                plan.from.saturating_add(plan.size),
                0,
                EXACT,
                None,
                TopOptions::default(),
            )?;
            if plan.sort.is_some() {
                top.hits.iter_mut().for_each(|h| h.score = None);
            }
            let mut before = top.hits;
            let hits = before.split_off(plan.from.min(before.len()));
            let offsets = offsets(plan, shard, segments, norms, hit.doc, &hits)?;
            let matched = child_names(plan, segments, norms, &hits, cx, (index, &shard.meta.uuid))?;
            results.push(InnerResult {
                name: plan.name.clone(),
                path: plan.path.clone(),
                total: top.total,
                max_score: top.max_score,
                hits,
                offsets,
                source_skip: plan.source_skip,
                matched,
            });
        }
        out.push(ParentInner {
            doc: hit.doc,
            results,
        });
    }
    Ok(out)
}

/// The names of the queries inside `plan`'s nested query that each of the
/// `hits` (its children) matches; empty when none is named.
fn child_names(
    plan: &InnerPlan,
    segments: &[OpenSegment<'_>],
    norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
    hits: &[Hit],
    cx: &QueryContext<'_>,
    (index, uuid): (&str, &str),
) -> Result<Vec<Vec<(String, f32)>>, ShardError> {
    let Some(query) = &plan.named else {
        // ALLOC: empty.
        return Ok(Vec::new());
    };
    if hits.is_empty() {
        // ALLOC: empty.
        return Ok(Vec::new());
    }
    let searcher = lucene_search::index_searcher::IndexSearcher::new(segments, norms)
        .map_err(ShardError::engine)?;
    let mut names = crate::search_named::matched(&searcher, &[query], cx, hits)
        .map_err(|f| ShardError::of_query(crate::lucene_query::shard_failure(f, index, uuid)))?;
    for list in &mut names {
        for (_, score) in list.iter_mut() {
            *score = f32::NAN;
        }
    }
    Ok(names)
}

/// The most nested documents one document holds (`index.mapping.nested_objects.limit`).
const MAX_NESTED_OBJECTS: usize = 10_000;

/// Each of `hits`' position among the children of `plan`'s path of `parent`:
/// the path's children in document order, which is the order of the array
/// the source holds them in.
fn offsets<S: BlobStore + 'static>(
    plan: &InnerPlan,
    shard: &OpenShard<S>,
    segments: &[OpenSegment<'_>],
    norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
    parent: i32,
    hits: &[Hit],
) -> Result<Vec<usize>, ShardError> {
    if hits.is_empty() {
        // ALLOC: empty.
        return Ok(Vec::new());
    }
    let query = BooleanQuery {
        // ALLOC: one clause.
        filter: vec![
            ParentChildrenBlockJoinQuery::new(
                Arc::clone(&plan.parents) as _,
                type_filter(&plan.path),
                parent,
            )
            .into(),
        ],
        ..BooleanQuery::default()
    };
    let all = collect_top(
        segments,
        shard,
        norms,
        &query,
        None,
        MAX_NESTED_OBJECTS,
        0,
        EXACT,
        None,
        TopOptions::default(),
    )?;
    // ALLOC: one document a child of the path, at most MAX_NESTED_OBJECTS.
    let mut docs: Vec<i32> = all.hits.iter().map(|h| h.doc).collect();
    docs.sort_unstable();
    // ALLOC: one offset a returned child, at most the inner window.
    Ok(hits
        .iter()
        .map(|h| docs.binary_search(&h.doc).unwrap_or(0))
        // ALLOC: one offset a returned child, at most the inner window.
        .collect())
}
