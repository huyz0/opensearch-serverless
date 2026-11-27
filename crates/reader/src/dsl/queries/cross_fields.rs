//! `multi_match` of type `cross_fields`: `MultiMatchQuery.buildCrossFieldQuery`
//! with its `BlendedQueryBuilder` and `blendTerms` at OpenSearch 3.10.0.
//! Fields that share an analyser are one group, whose terms are blended over
//! its fields as one term of one field would score; a group of one field is
//! a plain `match`, and the groups combine as a disjunction max.

use lucene_search::extended_query::{BlendedRewrite, BlendedTermQuery, ExtendedQuery};
use lucene_search::query_visitor::Occur;
use lucene_search::{BoostQuery, Clause, DisjunctionMaxQuery};

use crate::dsl::queries::full_text::MultiMatch;
use crate::dsl::query::Operator;
use crate::dsl::values::Value;
use crate::failure::Failure;
use crate::lucene_query::{
    QueryContext, apply_min_should_match, match_all, match_none, match_query, term, text_query,
    unsupported,
};
use crate::mapping::FieldType;

/// What a field is analysed by, as far as grouping goes: text fields by
/// their pinned analyser (none for the standard one), every other type by
/// the keyword analyser.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Analysis<'a> {
    Text(Option<&'a str>),
    Keyword,
}

fn boosted(query: Clause, boost: f32) -> Clause {
    if boost == 1.0 || matches!(query, Clause::MatchNoDocs(_)) {
        query
    } else {
        // ALLOC: one box a boosted field.
        Clause::Boost(Box::new(BoostQuery::new(query, boost)))
    }
}

/// `blendTerms`: `text` as a term of every field of the group, the term
/// queries blended and any other query beside them.
fn blend(
    cx: &QueryContext<'_>,
    fields: &[(&str, f32)],
    text: &str,
    tie_breaker: f32,
    lenient: bool,
) -> Result<Clause, Failure> {
    // ALLOC: one entry a field of the group, at most the mapping's fields.
    let mut terms: Vec<(String, Vec<u8>, f32)> = Vec::new();
    // ALLOC: one entry a field of the group, as above.
    let mut queries: Vec<Clause> = Vec::new();
    for (field, boost) in fields {
        let built = match term(cx, field, &Value::Str(text.to_owned())) {
            Ok(q) => q,
            Err(_) if lenient => match_none(),
            Err(e) => return Err(e),
        };
        match built {
            // ALLOC: the field name and the term's bytes, once a blended term.
            Clause::Term(t) => terms.push((t.field, t.term, *boost)),
            other => queries.push(boosted(other, *boost)),
        }
    }
    if !terms.is_empty() {
        let blended = BlendedTermQuery::new(terms, BlendedRewrite::DisjunctionMax(tie_breaker))
            // ALLOC: error path; one message.
            .map_err(|e| Failure::plain("illegal_argument_exception", e.to_string()))?;
        // ALLOC: one box a blended query.
        queries.push(Clause::Extended(Box::new(ExtendedQuery::Blended(blended))));
    }
    Ok(match queries.len() {
        1 => queries.pop().unwrap_or_else(match_none),
        // ALLOC: one box a disjunction of the blend and the other queries.
        _ => Clause::DisjunctionMax(Box::new(DisjunctionMaxQuery::new(queries, tie_breaker))),
    })
}

/// A group of several fields: the text's analysed terms, each blended.
fn group_query(
    cx: &QueryContext<'_>,
    m: &MultiMatch,
    analysis: Analysis<'_>,
    fields: &[(&str, f32)],
    tie_breaker: f32,
    lenient: bool,
) -> Result<Option<Clause>, Failure> {
    let text = m.query.text();
    let Analysis::Text(_) = analysis else {
        return if text.is_empty() {
            Ok(None)
        } else {
            blend(cx, fields, text, tie_breaker, lenient).map(Some)
        };
    };
    let occur = match m.operator {
        Operator::Or => Occur::Should,
        Operator::And => Occur::Must,
    };
    let Some((first, _)) = fields.first() else {
        return Ok(None);
    };
    let Some(clause) = text_query(cx, first, text, occur, false, 0)? else {
        return Ok(None);
    };
    let swap = |c: Clause| match c {
        Clause::Term(t) => blend(
            cx,
            fields,
            &String::from_utf8_lossy(&t.term),
            tie_breaker,
            lenient,
        ),
        _ => Err(unsupported(
            "[multi_match] [cross_fields] of a text with synonyms",
        )),
    };
    match clause {
        Clause::Boolean(mut b) => {
            for list in [&mut b.must, &mut b.should] {
                for c in std::mem::take(list) {
                    list.push(swap(c)?);
                }
            }
            // ALLOC: one box a group's terms.
            let b = apply_min_should_match(*b, m.minimum_should_match.as_deref())?;
            Ok(Some(Clause::Boolean(Box::new(b))))
        }
        single => swap(single).map(Some),
    }
}

/// `MultiMatchQuery.parse` of type `CROSS_FIELDS` over the resolved `fields`.
///
/// # Errors
///
/// A failure of a field's query, or this node's refusal of a text with synonyms.
pub fn build(
    cx: &QueryContext<'_>,
    m: &MultiMatch,
    fields: &[(&str, f32)],
    tie_breaker: f32,
) -> Result<Clause, Failure> {
    let lenient = m
        .lenient
        .unwrap_or_else(|| m.fields.iter().any(|(f, _)| f == "*"));
    // ALLOC: one group an analyser, at most one a resolved field.
    let mut groups: Vec<(Analysis<'_>, Vec<(&str, f32)>)> = Vec::new();
    for (field, boost) in fields {
        let Some(f) = cx.mapping.get(field) else {
            continue;
        };
        let analysis = match f.ty {
            FieldType::Text => Analysis::Text(f.analyzer.as_deref()),
            _ => Analysis::Keyword,
        };
        match groups.iter_mut().find(|(a, _)| *a == analysis) {
            Some((_, members)) => members.push((field, *boost)),
            // ALLOC: one entry a group, at most one a resolved field.
            None => groups.push((analysis, vec![(*field, *boost)])),
        }
    }
    // ALLOC: one query a group, at most one a resolved field.
    let mut queries: Vec<Clause> = Vec::new();
    for (analysis, members) in &groups {
        let query = match members.as_slice() {
            [(field, boost)] => {
                let built = match_query(
                    cx,
                    field,
                    &m.query,
                    m.operator,
                    m.minimum_should_match.as_deref(),
                    m.zero_terms_all,
                );
                let built = match built {
                    Ok(q) => q,
                    Err(_) if lenient => match_none(),
                    Err(e) => return Err(e),
                };
                Some(boosted(built, *boost))
            }
            _ => group_query(cx, m, *analysis, members, tie_breaker, lenient)?,
        };
        queries.extend(query);
    }
    Ok(match queries.len() {
        0 => {
            if m.zero_terms_all {
                match_all(cx)
            } else {
                match_none()
            }
        }
        1 => queries.pop().unwrap_or_else(match_none),
        // ALLOC: one box the groups' disjunction.
        _ => Clause::DisjunctionMax(Box::new(DisjunctionMaxQuery::new(queries, tie_breaker))),
    })
}

#[cfg(test)]
#[path = "cross_fields_tests.rs"]
mod tests;
