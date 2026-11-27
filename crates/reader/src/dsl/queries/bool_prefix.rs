//! `match_bool_prefix`: `MatchBoolPrefixQueryBuilder.fromXContent` and
//! `doToQuery` with `MatchQuery.parse(BOOLEAN_PREFIX)` at OpenSearch 3.10.0:
//! the text's terms are term queries, joined by the operator, and the last
//! is a prefix query.

use lucene_search::Clause;
use lucene_search::query_visitor::Occur;

use crate::dsl::error::DslError;
use crate::dsl::queries::More;
use crate::dsl::queries::build::prefix_clause;
use crate::dsl::query::{Operator, does_not_support, multiple_fields, next_or_end, parse_operator};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{Value, bool_value, float_value, int_value, object_value, text_or_null};
use crate::failure::Failure;
use crate::lucene_query::{
    QueryContext, apply_min_should_match, field_type, match_none, term, text_query, unsupported,
};
use crate::mapping::FieldType;

/// The parameters of a `match_bool_prefix` query.
#[derive(Clone, Debug, PartialEq)]
pub struct BoolPrefix {
    /// The field.
    pub field: String,
    /// The text.
    pub query: Value,
    /// How the terms combine.
    pub operator: Operator,
    /// `minimum_should_match`.
    pub minimum_should_match: Option<String>,
}

/// `MatchBoolPrefixQueryBuilder.fromXContent`.
pub fn parse(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut field: Option<&str> = None;
    let mut value = None;
    let mut boost = 1.0;
    let mut operator = Operator::Or;
    let mut minimum_should_match = None;
    let mut current = "";
    while let Some(token) = next_or_end(p, Kind::EndObject) {
        match token {
            Kind::FieldName => current = p.text(),
            Kind::StartObject => {
                if let Some(f) = field {
                    return Err(multiple_fields(p, "match_bool_prefix", f, current));
                }
                field = Some(current);
                while let Some(token) = next_or_end(p, Kind::EndObject) {
                    if token == Kind::FieldName {
                        current = p.text();
                        continue;
                    }
                    if !token.is_value() {
                        return Err(DslError::parsing(
                            p.location(),
                            // ALLOC: error path; one message.
                            format!(
                                "[match_bool_prefix] unknown token [{}] after [{current}]",
                                token.name()
                            ),
                        ));
                    }
                    match current {
                        "query" => value = object_value(p)?,
                        "boost" => boost = float_value(p)?,
                        "operator" => operator = parse_operator(p.text())?,
                        // ALLOC: one string a clause, bounded by the body's recording.
                        "minimum_should_match" => {
                            minimum_should_match = text_or_null(p).map(str::to_owned);
                        }
                        "analyzer" if p.text() == "standard" => {}
                        // Read, and changing nothing without a `fuzziness`.
                        "prefix_length" | "max_expansions" => {
                            int_value(p)?;
                        }
                        "fuzzy_transpositions" => {
                            bool_value(p)?;
                        }
                        "analyzer" | "fuzziness" | "fuzzy_rewrite" | "_name" => {
                            // ALLOC: error path; one message.
                            return Err(DslError::unsupported(&format!(
                                "[match_bool_prefix] [{current}]"
                            )));
                        }
                        _ => return Err(does_not_support(p, "match_bool_prefix", current)),
                    }
                }
            }
            _ => {
                let name = p.current_name().unwrap_or("");
                if let Some(f) = field {
                    return Err(multiple_fields(p, "match_bool_prefix", f, name));
                }
                field = Some(name);
                value = object_value(p)?;
            }
        }
    }
    let Some(field) = field.filter(|f| !f.is_empty()) else {
        return Err(DslError::illegal("[match_bool_prefix] requires fieldName"));
    };
    let Some(query) = value else {
        return Err(DslError::illegal(
            "[match_bool_prefix] requires query value",
        ));
    };
    let m = BoolPrefix {
        // ALLOC: one field name a clause, bounded by the body's recording.
        field: field.to_owned(),
        query,
        operator,
        minimum_should_match,
    };
    // ALLOC: one box a clause, bounded by the body's recording.
    Ok(More::MatchBoolPrefix(Box::new(m), boost))
}

/// `MatchQuery.parse(BOOLEAN_PREFIX, ...)` then `maybeApplyMinimumShouldMatch`.
///
/// # Errors
///
/// A failure of the text's analysis, or this node's refusal of a text that
/// ends in something other than a letter or digit, where OpenSearch's
/// choice of last term depends on the analyser's offsets.
pub fn build(cx: &QueryContext<'_>, m: &BoolPrefix) -> Result<Clause, Failure> {
    let field = m.field.as_str();
    let Some(ty) = field_type(cx, field)? else {
        return Ok(match_none());
    };
    match ty {
        FieldType::Text => {}
        FieldType::Keyword => return prefix_clause(cx, field, m.query.text()),
        _ => return term(cx, field, &m.query),
    }
    let occur = match m.operator {
        Operator::Or => Occur::Should,
        Operator::And => Occur::Must,
    };
    let Some(clause) = text_query(cx, field, m.query.text(), occur, false, 0)? else {
        return Ok(match_none());
    };
    if !m
        .query
        .text()
        .chars()
        .last()
        .is_some_and(char::is_alphanumeric)
    {
        return Err(unsupported(
            "[match_bool_prefix] of a text that ends in a character that is not a letter or digit",
        ));
    }
    let to_prefix = |c: Clause| match c {
        Clause::Term(t) => prefix_clause(cx, field, &String::from_utf8_lossy(&t.term)),
        _ => Err(unsupported("[match_bool_prefix] of a text with synonyms")),
    };
    match clause {
        Clause::Boolean(mut b) => {
            let list = match m.operator {
                Operator::Or => &mut b.should,
                Operator::And => &mut b.must,
            };
            if let Some(last) = list.pop() {
                list.push(to_prefix(last)?);
            }
            // ALLOC: one box a match query's terms.
            Ok(Clause::Boolean(Box::new(apply_min_should_match(
                *b,
                m.minimum_should_match.as_deref(),
            )?)))
        }
        single => to_prefix(single),
    }
}

#[cfg(test)]
#[path = "bool_prefix_tests.rs"]
mod tests;
