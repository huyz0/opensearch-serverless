//! `match_phrase` and `multi_match` as Lucene queries: `MatchQuery.parse`
//! of the phrase type, `MultiMatchQuery.parse` with `buildFieldQueries` and
//! `combineGrouped` (server/src/main/java/org/opensearch/index/search/),
//! and `QueryParserHelper.resolveMappingFields`
//! (server/src/main/java/org/opensearch/index/search/QueryParserHelper.java),
//! at OpenSearch 3.10.0.

use std::collections::HashMap;

use lucene_search::query_builder::QueryBuilder;
use lucene_search::{BoostQuery, Clause, DisjunctionMaxQuery};

use crate::dsl::queries::build::simple_match;
use crate::dsl::queries::full_text::{MultiMatch, MultiType};
use crate::dsl::values::Value;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, field_type, match_all, match_none, match_query, term};
use crate::mapping::FieldType;

fn zero_terms(cx: &QueryContext<'_>, all: bool) -> Clause {
    if all { match_all(cx) } else { match_none() }
}

/// `MatchQuery.parse(Type.PHRASE, ...)`: a text field's phrase, any other
/// field's term query.
pub fn phrase(
    cx: &QueryContext<'_>,
    field: &str,
    value: &Value,
    slop: u32,
    zero_terms_all: bool,
) -> Result<Clause, Failure> {
    let Some(ty) = field_type(cx, field)? else {
        return Ok(match_none());
    };
    if *ty != FieldType::Text {
        return term(cx, field, value);
    }
    let built = QueryBuilder::new(cx.analyzer)
        .create_phrase_query_with_slop(field, value.text(), slop)
        .map_err(|e| {
            // ALLOC: error path; one message.
            Failure::plain("illegal_argument_exception", e.to_string())
        })?;
    Ok(built.unwrap_or_else(|| zero_terms(cx, zero_terms_all)))
}

/// `QueryParserHelper.resolveMappingFields`: each name the later boost it
/// was listed with (`fieldsBoosts.put`), patterns expanded over the mapping
/// (types it cannot search left out), unmapped names dropped, and a field
/// two entries resolve to taking the product of their boosts. Java's check
/// for more fields than the clause limit (1,024) cannot fail here: a
/// mapping holds at most `mapping::MAX_FIELDS` (1,000).
fn resolve<'a>(
    cx: &'a QueryContext<'_>,
    listed: &'a [(String, f32)],
) -> Result<Vec<(&'a str, f32)>, Failure> {
    // ALLOC: one entry a listed name, at most MAX_LISTED_FIELDS.
    let mut last: HashMap<&str, f32> = HashMap::with_capacity(listed.len());
    for (name, boost) in listed {
        last.insert(name.as_str(), *boost);
    }
    // ALLOC: one entry a resolved field, at most the mapping's fields.
    let mut out: Vec<(&'a str, f32)> = Vec::new();
    // ALLOC: one entry a resolved field, at most the mapping's fields.
    let mut at: HashMap<&str, usize> = HashMap::new();
    let mut add = |name: &'a str, boost: f32| match at.get(name) {
        Some(&i) => {
            if let Some(entry) = out.get_mut(i) {
                entry.1 *= boost;
            }
        }
        None => {
            at.insert(name, out.len());
            out.push((name, boost));
        }
    };
    for (name, _) in listed {
        // A name listed twice is resolved once, with its later boost.
        let Some(boost) = last.remove(name.as_str()) else {
            continue;
        };
        if name.contains('*') {
            for field in cx.mapping.names() {
                let searchable = cx
                    .mapping
                    .get(field)
                    .is_some_and(|f| !matches!(f.ty, FieldType::Unsupported(_)));
                if searchable && simple_match(name, field) {
                    add(field, boost);
                }
            }
        } else if cx.mapping.get(name).is_some() {
            add(name, boost);
        }
    }
    Ok(out)
}

/// `MultiMatchQuery.parse` for the types served.
pub fn multi_match(cx: &QueryContext<'_>, m: &MultiMatch) -> Result<Clause, Failure> {
    let fields = resolve(cx, &m.fields)?;
    if fields.is_empty() {
        return Ok(match_none());
    }
    let all_fields = m.fields.iter().any(|(f, _)| f == "*");
    let lenient = m.lenient.unwrap_or(all_fields);
    // ALLOC: one entry a resolved field, at most the mapping's fields.
    let mut queries = Vec::new();
    for (field, boost) in fields {
        let built = match m.kind {
            MultiType::Phrase => phrase(cx, field, &m.query, m.slop, m.zero_terms_all),
            MultiType::BestFields | MultiType::MostFields => match_query(
                cx,
                field,
                &m.query,
                m.operator,
                m.minimum_should_match.as_deref(),
                m.zero_terms_all,
            ),
        };
        let query = match built {
            Ok(q) => q,
            // `newLenientFieldQuery`: the field matches nothing.
            Err(_) if lenient => match_none(),
            Err(e) => return Err(e),
        };
        let query = if boost != 1.0 && !matches!(query, Clause::MatchNoDocs(_)) {
            // ALLOC: one box a boosted field.
            Clause::Boost(Box::new(BoostQuery::new(query, boost)))
        } else {
            query
        };
        queries.push(query);
    }
    let tie_breaker = m.tie_breaker.unwrap_or_else(|| m.kind.tie_breaker());
    // At least one field resolved, and each adds a query.
    Ok(match queries.len() {
        1 => queries.pop().unwrap_or_else(match_none),
        // ALLOC: one box a multi_match over several fields.
        _ => Clause::DisjunctionMax(Box::new(DisjunctionMaxQuery::new(queries, tie_breaker))),
    })
}
