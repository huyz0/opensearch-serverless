//! Running the `knn` queries of a search.
//!
//! A `knn` query is Lucene's `AbstractKnnVectorQuery.rewrite`: the best `k`
//! of the whole shard, the filter applied while the graph is walked, the
//! answer a fixed set of documents with their scores. So it is computed
//! once, over the shard's open segments, before the query tree is built; the
//! builder then takes each node's answer from [`KnnResults`] by the node's
//! identity.

use std::collections::HashMap;

use lucene_search::Clause;
use lucene_search::extended_query::{DocAndScoreQuery, ExtendedQuery};
use lucene_search::knn_search::{KnnTarget, knn_top_hits};
use lucene_search::multi_segment::OpenSegment;

use crate::dsl::Query;
use crate::dsl::queries::More;
use crate::dsl::queries::knn::Knn;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, build};

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
                More::Knn(k) => out.push(k),
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

fn engine(e: impl std::fmt::Display) -> Failure {
    // ALLOC: error path; one message.
    Failure::plain("illegal_state_exception", e.to_string())
}

fn illegal(reason: String) -> Failure {
    Failure::plain("illegal_argument_exception", reason)
}

/// Checks `knn` against the mapping, as the plugin's `KNNQueryBuilder.doToQuery`.
fn check(knn: &Knn, cx: &QueryContext<'_>) -> Result<(), Failure> {
    let dimension = match cx.mapping.get(&knn.field).and_then(|f| f.knn_dimension) {
        Some(dimension) => dimension,
        // ALLOC: error path; one message.
        None => {
            return Err(illegal(format!(
                "Field '{}' is not knn_vector type.",
                knn.field
            )));
        }
    };
    if usize::try_from(dimension).ok() != Some(knn.vector.len()) {
        // ALLOC: error path; one message.
        return Err(illegal(format!(
            "Query vector has invalid dimension: {}. Dimension should be: {dimension}",
            knn.vector.len()
        )));
    }
    Ok(())
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
        check(knn, cx)?;
        let filter = match &knn.filter {
            Some(f) => Some(build(f, cx)?),
            None => None,
        };
        let hits = knn_top_hits(
            segments,
            &knn.field,
            &KnnTarget::Float(knn.vector.clone()),
            knn.k,
            knn.k,
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
mod tests;
