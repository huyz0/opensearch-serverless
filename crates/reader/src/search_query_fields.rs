//! What a query reads: whether it has a point clause, and the text fields it scores.

use lucene_search::extended_query::ExtendedQuery;
use lucene_search::{BooleanQuery, Clause};

/// Whether a query has a point clause, which needs the points open.
pub(crate) fn has_points(query: &BooleanQuery) -> bool {
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
            | Clause::MultiPhrase(lucene_search::query::MultiPhraseQuery { field, .. })
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
                    // Its expansion is a multi-phrase over the same field.
                    ExtendedQuery::MultiPhrasePrefix(q) if !out.contains(&q.field) => {
                        // ALLOC: one name a scored field, at most the query's terms.
                        out.push(q.field.clone());
                    }
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
