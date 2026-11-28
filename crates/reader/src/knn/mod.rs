//! Running the `knn` queries of a search.
//!
//! A `knn` query is Lucene's `AbstractKnnVectorQuery.rewrite`: the best `k`
//! of the whole shard, the filter applied while the graph is walked, the
//! answer a fixed set of documents with their scores. So it is computed
//! once, over the shard's open segments, before the query tree is built; the
//! builder then takes each node's answer from [`KnnResults`] by the node's
//! identity.

use std::collections::HashMap;
use std::sync::Arc;

use lucene_search::Clause;
use lucene_search::extended_query::{DocAndScoreQuery, DocumentClause, ExtendedQuery};
use lucene_search::knn_search::{KnnLeafKind, KnnLeafQuery, KnnTarget, knn_top_hits};
use lucene_search::multi_segment::OpenSegment;

use crate::dsl::Query;
use crate::dsl::queries::More;
use crate::dsl::queries::knn::{Knn, Mode};
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, build};
use crate::mapping::{KnnField, Space};

pub mod sample;
pub mod stats;

/// The answer of each `knn` node of a query.
#[derive(Default)]
pub struct KnnResults {
    by_node: HashMap<usize, Clause>,
}

impl KnnResults {
    /// The answer of `node`, `None` for a node that was not run.
    pub fn get(&self, node: &Knn) -> Option<Clause> {
        self.by_node.get(&std::ptr::from_ref(node).addr()).cloned()
    }
}

/// Every `knn` node of `query` that is run before the tree is built: those
/// reached through `bool`, `constant_score`, `dis_max` and `function_score`'s
/// query. A node anywhere else is refused when the tree is built.
pub fn nodes(query: &Query) -> Vec<&Knn> {
    fn walk<'q>(q: &'q Query, out: &mut Vec<&'q Knn>) {
        match q {
            Query::Named(n) => walk(&n.query, out),
            Query::Bool(b) => {
                for c in b
                    .must
                    .iter()
                    .chain(&b.should)
                    .chain(&b.must_not)
                    .chain(&b.filter)
                {
                    walk(c, out);
                }
            }
            Query::More(m) => match m.as_ref() {
                More::Knn(k) if matches!(k.mode, Mode::TopK(_)) => out.push(k),
                More::ConstantScore { filter, .. } => walk(filter, out),
                More::DisMax { queries, .. } => queries.iter().for_each(|q| walk(q, out)),
                More::FunctionScore(f, _) => {
                    if let Some(q) = &f.query {
                        walk(q, out);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    // ALLOC: one entry a knn query, at most the query's clauses.
    let mut out = Vec::new();
    walk(query, &mut out);
    out
}

/// Whether `query` holds a `knn` query of either mode where a search reaches
/// one: the places [`nodes`] looks. A search with one reads vectors, so the
/// vector files are brought in before it starts.
fn any(query: &Query) -> bool {
    match query {
        Query::Named(n) => any(&n.query),
        Query::Bool(b) => b
            .must
            .iter()
            .chain(&b.should)
            .chain(&b.must_not)
            .chain(&b.filter)
            .any(any),
        Query::More(m) => match m.as_ref() {
            More::Knn(_) => true,
            More::ConstantScore { filter, .. } => any(filter),
            More::DisMax { queries, .. } => queries.iter().any(any),
            More::FunctionScore(f, _) => f.query.as_ref().is_some_and(any),
            _ => false,
        },
        _ => false,
    }
}

fn engine(e: impl std::fmt::Display) -> Failure {
    // ALLOC: error path; one message.
    Failure::plain("illegal_state_exception", e.to_string())
}

fn illegal(reason: String) -> Failure {
    Failure::plain("illegal_argument_exception", reason)
}

/// Whether `knn` names a field the mapping lacks and asked to ignore that:
/// it then matches nothing and is not checked further.
fn ignored(knn: &Knn, cx: &QueryContext<'_>) -> bool {
    knn.ignore_unmapped && cx.mapping.get(&knn.field).is_none()
}

/// A query that matches no document.
fn nothing(bases: &[i32]) -> Clause {
    // ALLOC: one box a knn query.
    Clause::Extended(Box::new(ExtendedQuery::DocAndScore(DocAndScoreQuery::new(
        Vec::new(),
        bases,
    ))))
}

/// Checks `knn` against the mapping, as the plugin's `KNNQueryBuilder.doToQuery`.
fn check(knn: &Knn, cx: &QueryContext<'_>) -> Result<KnnField, Failure> {
    let field = match cx.mapping.get(&knn.field).and_then(|f| f.knn) {
        Some(field) => field,
        // ALLOC: error path; one message.
        None => {
            return Err(illegal(format!(
                "Field '{}' is not knn_vector type.",
                knn.field
            )));
        }
    };
    let dimension = field.dimension;
    if usize::try_from(dimension).ok() != Some(knn.vector.len()) {
        // ALLOC: error path; one message.
        return Err(illegal(format!(
            "Query vector has invalid dimension: {}. Dimension should be: {dimension}",
            knn.vector.len()
        )));
    }
    Ok(field)
}

/// Runs the `knn` queries of `query`, if it has any, over `shard`'s segments:
/// their answers and the segments, opened once for the search to reuse. A
/// `knn` query is the best `k` of the whole shard, so it is computed before
/// the query tree is built.
pub fn prepare<'s, S: oss_blob::BlobStore + 'static>(
    shard: &'s crate::OpenShard<S>,
    query: Option<&Query>,
    cx: &QueryContext<'_>,
) -> Result<
    Option<(
        KnnResults,
        lucene_search::directory_reader::OpenedSegments<'s>,
    )>,
    Failure,
> {
    let nodes = query.map(self::nodes).unwrap_or_default();
    // A query the mapping refuses reads nothing: validate before warming.
    for knn in nodes.iter().filter(|k| !ignored(k, cx)) {
        check(knn, cx)?;
    }
    if query.is_some_and(any) {
        crate::vector_warm::warm(shard);
    }
    if nodes.is_empty() {
        return Ok(None);
    }
    let mut segments = shard.reader.open_segments().map_err(engine)?;
    if nodes.iter().any(|k| k.filter.is_some()) {
        segments.open_points().map_err(engine)?;
    }
    let results = run(&nodes, &segments.as_open_segments(), cx)?;
    Ok(Some((results, segments)))
}

/// Runs each of `nodes` over `segments`.
pub fn run(
    nodes: &[&Knn],
    segments: &[OpenSegment<'_>],
    cx: &QueryContext<'_>,
) -> Result<KnnResults, Failure> {
    let mut results = KnnResults::default();
    // ALLOC: one entry a segment.
    let bases: Vec<i32> = segments.iter().map(|s| s.doc_base).collect();
    for knn in nodes {
        let Mode::TopK(k) = knn.mode else {
            check(knn, cx)?;
            continue;
        };
        if ignored(knn, cx) {
            results
                .by_node
                .insert(std::ptr::from_ref(*knn).addr(), nothing(&bases));
            continue;
        }
        check(knn, cx)?;
        stats::record(knn.filter.is_some());
        let filter = match &knn.filter {
            Some(f) => Some(build(f, cx)?),
            None => None,
        };
        let hits = knn_top_hits(
            segments,
            &knn.field,
            &KnnTarget::Float(knn.vector.clone()),
            k,
            k,
            filter.as_ref(),
        )
        .map_err(engine)?;
        // ALLOC: one entry a hit, at most k.
        let pairs = hits.into_iter().map(|h| (h.doc_id, h.score)).collect();
        let query = DocAndScoreQuery::new(pairs, &bases);
        results.by_node.insert(
            std::ptr::from_ref(*knn).addr(),
            // ALLOC: one box a knn query.
            Clause::Extended(Box::new(ExtendedQuery::DocAndScore(query))),
        );
    }
    Ok(results)
}

#[cfg(test)]
mod sample_tests;
#[cfg(test)]
mod stats_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod unmapped_tests;

/// The score a document must reach for `mode` in `space`, as the plugin
/// translates a distance to the space's Lucene score.
fn threshold(mode: Mode, space: Space) -> Result<f32, Failure> {
    let name = space.name();
    match (mode, space) {
        (Mode::MinScore(score), Space::L2 | Space::CosineSimil) if score > 1.0 => Err(illegal(
            format!("[knn] requires score to be in the range [0, 1] for space type: {name}"),
        )),
        (Mode::MinScore(score), _) => Ok(score),
        (Mode::MaxDistance(d), Space::L2 | Space::CosineSimil) if d < 0.0 => Err(illegal(format!(
            "[knn] requires distance to be non-negative for space type: {name}"
        ))),
        (Mode::MaxDistance(d), Space::L2) => Ok(1.0 / (1.0 + d)),
        (Mode::MaxDistance(d), Space::CosineSimil) => Ok((2.0 - d) / 2.0),
        // The distance is the inner product itself, as the space scores it:
        // one plus a positive product, one over one minus a negative one.
        (Mode::MaxDistance(d), Space::InnerProduct) if d >= 0.0 => Ok(d + 1.0),
        (Mode::MaxDistance(d), Space::InnerProduct) => Ok(1.0 / (1.0 - d)),
        (Mode::TopK(_), _) => Err(illegal("[knn] a top-k query has no threshold".to_owned())),
    }
}

/// A `knn` query of `min_score` or `max_distance`: every document scoring at
/// least the threshold, found segment by segment, `filter` applied while the
/// graph is walked.
pub(crate) fn radial(knn: &Knn, cx: &QueryContext<'_>) -> Result<Clause, Failure> {
    if ignored(knn, cx) {
        return Ok(nothing(&[]));
    }
    let field = check(knn, cx)?;
    let min_score = threshold(knn.mode, field.space)?;
    stats::record(knn.filter.is_some());
    let leaf = KnnLeafQuery::new(
        knn.field.clone(),
        KnnTarget::Float(knn.vector.clone()),
        KnnLeafKind::Radial { min_score },
    )
    .map_err(engine)?;
    // ALLOC: one key a radial query, from the field and the vector.
    let key = format!("knn:{}:{:?}:{:?}", knn.field, knn.mode, knn.vector);
    // ALLOC: one box a radial query.
    let mut clause = DocumentClause::new(key, Arc::new(leaf));
    if let Some(filter) = &knn.filter {
        clause = clause.with_filter(build(filter, cx)?);
    }
    // ALLOC: one box a radial query.
    Ok(Clause::Extended(Box::new(ExtendedQuery::Document(clause))))
}
