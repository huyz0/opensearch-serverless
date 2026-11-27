//! `matched_queries`: which of the named queries of a search match each hit,
//! as `MatchedQueriesPhase` (server/src/main/java/org/opensearch/search/fetch/
//! subphase/) answers it at OpenSearch 3.10.0. Each named query is built on
//! its own and scored at the hits' documents only.

use lucene_search::index_searcher::IndexSearcher;

use crate::dsl::Query;
use crate::dsl::queries::More;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, to_lucene};
use crate::search::Hit;

/// The most named queries a search may hold.
pub const MAX_NAMED: usize = 64;

/// The named queries of `q`, in the order their names appear; a name used
/// twice keeps its first query.
pub fn collect(q: &Query) -> Vec<(&str, &Query)> {
    fn walk<'q>(q: &'q Query, out: &mut Vec<(&'q str, &'q Query)>) {
        match q {
            Query::Named(n) => {
                if !out.iter().any(|(name, _)| *name == n.name) {
                    // ALLOC: one entry a named query, bounded by `MAX_NAMED` where used.
                    out.push((&n.name, &n.query));
                }
                walk(&n.query, out);
            }
            Query::Bool(b) => {
                for c in b
                    .must
                    .iter()
                    .chain(&b.should)
                    .chain(&b.filter)
                    .chain(&b.must_not)
                {
                    walk(c, out);
                }
            }
            Query::More(m) => match &**m {
                More::ConstantScore { filter, .. } => walk(filter, out),
                More::DisMax { queries, .. } => queries.iter().for_each(|q| walk(q, out)),
                More::FunctionScore(f, _) => {
                    if let Some(c) = &f.query {
                        walk(c, out);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    // ALLOC: one entry a named query, refused past `MAX_NAMED` by `matched`.
    let mut out = Vec::new();
    walk(q, &mut out);
    out
}

/// The named queries of every query of a search, the first of a name kept.
pub fn collect_all<'q>(queries: &[&'q Query]) -> Vec<(&'q str, &'q Query)> {
    // ALLOC: one entry a named query, refused past `MAX_NAMED` by `matched`.
    let mut out: Vec<(&str, &Query)> = Vec::new();
    for q in queries {
        for (name, found) in collect(q) {
            if !out.iter().any(|(n, _)| *n == name) {
                out.push((name, found));
            }
        }
    }
    out
}

/// For each of `hits`, in order, the named queries of `queries` that match it
/// and the score each gives it; empty when the query names none.
///
/// # Errors
/// The failure of building a named query, and `illegal_argument_exception`
/// for more than [`MAX_NAMED`] names.
pub fn matched(
    searcher: &IndexSearcher<'_, '_>,
    queries: &[&Query],
    cx: &QueryContext<'_>,
    hits: &[Hit],
) -> Result<Vec<Vec<(String, f32)>>, Failure> {
    // ALLOC: one entry a named query, refused past `MAX_NAMED` below.
    let named = collect_all(queries);
    if named.is_empty() {
        return Ok(Vec::new());
    }
    if named.len() > MAX_NAMED {
        return Err(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "too many named queries [{}]; the limit is {MAX_NAMED}",
                named.len()
            ),
        ));
    }
    // The engine scores ascending documents; hits may come in score order.
    // ALLOC: one entry a hit, at most the result window.
    let mut order: Vec<usize> = (0..hits.len()).collect();
    order.sort_by_key(|&i| hits[i].doc);
    // ALLOC: one id a hit, at most the result window.
    let docs: Vec<i32> = order.iter().map(|&i| hits[i].doc).collect();
    // ALLOC: one list a hit, at most the result window.
    let mut out: Vec<Vec<(String, f32)>> = vec![Vec::new(); hits.len()];
    for (name, q) in named {
        let lucene = to_lucene(q, cx)?;
        let scores = searcher
            .scores_at(&lucene, &docs)
            // ALLOC: error path; one message.
            .map_err(|e| Failure::plain("illegal_state_exception", e.to_string()))?;
        for (&at, score) in order.iter().zip(scores) {
            if let Some(score) = score {
                // ALLOC: one name a match, at most `MAX_NAMED` a hit.
                out[at].push((name.to_owned(), score));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "search_named_tests.rs"]
mod tests;
