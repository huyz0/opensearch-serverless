//! `explain`: each hit carries the explanation of its score, as
//! `ExplainFetchSubPhase` and `SearchHit.toInnerXContent` give it at
//! OpenSearch 3.10.0. The engine explains a document against the query as a
//! search scored it; this module collects those explanations and writes
//! them as the hit's `_explanation` object.

use std::io::{self, Write};

use lucene_search::index_searcher::IndexSearcher;
use lucene_search::{BooleanQuery, Clause, DisjunctionMaxQuery, Explanation};
use oss_rest::write_json_str;

use crate::search::Hit;

/// What of `query` the engine does not explain as OpenSearch does, for the
/// refusal "explain of X is not supported by this node yet".
pub fn refusal(query: &BooleanQuery) -> Option<&'static str> {
    fn clause(c: &Clause) -> Option<&'static str> {
        match c {
            Clause::Extended(_) | Clause::PointsRange(_) => Some("a range or other points query"),
            Clause::Exists(_) => Some("an exists query"),
            Clause::TermInSet(t) if t.field == "_id" => Some("an ids query"),
            Clause::Boost(b) if matches!(*b.inner, Clause::ConstantScore(_)) => {
                Some("a constant_score query with a boost")
            }
            Clause::Boost(b) => clause(&b.inner),
            Clause::ConstantScore(c) => clause(&c.inner),
            Clause::DisjunctionMax(d) => d.disjuncts.iter().find_map(clause),
            Clause::Boolean(b) => refusal(b),
            _ => None,
        }
    }
    query
        .must
        .iter()
        .chain(&query.should)
        .chain(&query.filter)
        .chain(&query.must_not)
        .find_map(clause)
}

/// `query` as Lucene's `rewrite` leaves it where that shows in an
/// explanation: a `DisjunctionMaxQuery` with a tie breaker of 1 is a
/// `BooleanQuery` of its disjuncts.
fn rewritten(query: &BooleanQuery) -> BooleanQuery {
    fn clause(c: &Clause) -> Clause {
        match c {
            Clause::DisjunctionMax(d) if d.tie_breaker == 1.0 => {
                // ALLOC: one box a rewritten disjunction, bounded by the query's recording.
                Clause::Boolean(Box::new(BooleanQuery {
                    should: d.disjuncts.iter().map(clause).collect(),
                    ..BooleanQuery::default()
                }))
            }
            // ALLOC: one box a dis_max, bounded by the query's recording.
            Clause::DisjunctionMax(d) => Clause::DisjunctionMax(Box::new(DisjunctionMaxQuery {
                disjuncts: d.disjuncts.iter().map(clause).collect(),
                tie_breaker: d.tie_breaker,
            })),
            // ALLOC: one box a nested bool, bounded by the query's recording.
            Clause::Boolean(b) => Clause::Boolean(Box::new(rewritten(b))),
            other => other.clone(),
        }
    }
    let mut q = query.clone();
    for list in [&mut q.must, &mut q.should, &mut q.filter, &mut q.must_not] {
        // ALLOC: one clause a clause of the query, bounded by its recording.
        *list = list.iter().map(clause).collect();
    }
    q
}

/// The explanation of each of `hits`, in order, for `query` over `segments`.
///
/// # Errors
/// The engine's message when a document cannot be explained.
pub fn explain_hits(
    searcher: &IndexSearcher<'_, '_>,
    query: &BooleanQuery,
    hits: &[Hit],
) -> Result<Vec<Explanation>, String> {
    let query = rewritten(query);
    hits.iter()
        .map(|h| {
            searcher
                .explain(&query, h.doc)
                .map(|e| as_opensearch_names_it(&e))
                // ALLOC: error path; one message.
                .map_err(|e| e.to_string())
        })
        // ALLOC: one explanation a hit, at most the result window.
        .collect()
}

/// The descriptions of `e` where the engine builds a different query from
/// the one OpenSearch does: a keyword's term query is already a constant
/// score here, so a `constant_score` over it, or a filter clause that
/// Lucene's rewrite unwraps, names the term query alone.
fn as_opensearch_names_it(e: &Explanation) -> Explanation {
    let mut out = e.clone();
    // ALLOC: one explanation a detail, bounded by the query's tree.
    out.details = e.details.iter().map(as_opensearch_names_it).collect();
    if let Some(inner) = out
        .description
        .strip_prefix("ConstantScore(ConstantScore(")
        .and_then(|d| d.strip_suffix("))"))
    {
        // ALLOC: one name, the description's own length.
        out.description = format!("ConstantScore({inner})");
    }
    let required = out.description == "match on required clause, product of:";
    if let Some(clause) = out.details.get_mut(1).filter(|_| required)
        && let Some(inner) = clause
            .description
            .strip_prefix("ConstantScore(")
            .and_then(|d| d.strip_suffix(')'))
    {
        // ALLOC: one name, the description's own length.
        clause.description = inner.to_owned();
    }
    out
}

/// Writes `,"_explanation":{...}` for one hit: the value as Java prints the
/// number the explanation holds, its description and its details.
pub fn write_explanation(out: &mut dyn Write, e: &Explanation) -> io::Result<()> {
    out.write_all(b",\"_explanation\":")?;
    write_node(out, e)
}

fn write_node(out: &mut dyn Write, e: &Explanation) -> io::Result<()> {
    write!(out, "{{\"value\":{},\"description\":", e.value_string())?;
    // OpenSearch scores through its per-field similarity, which names itself.
    write_json_str(
        out,
        &e.description
            .replace("[BM25Similarity]", "[PerFieldSimilarity]"),
    )?;
    out.write_all(b",\"details\":[")?;
    for (i, d) in e.details.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        write_node(out, d)?;
    }
    out.write_all(b"]}")
}

#[cfg(test)]
#[path = "explain_tests.rs"]
mod tests;
