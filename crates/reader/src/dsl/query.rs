//! The query builders of the subset and their parsers: `match_all`,
//! `term`, `match`, `range` and `bool`.
//!
//! Each parser is ported from its builder's `fromXContent` at OpenSearch
//! 3.10.0 (server/src/main/java/org/opensearch/index/query/), with the same
//! messages and locations; `parse_inner` is
//! `AbstractQueryBuilder.parseInnerQueryBuilder`. A parameter OpenSearch
//! takes but this reader does not serve yet is refused
//! ([`DslError::unsupported`]) rather than ignored.

use crate::dsl::bool_query::{BoolQuery, parse_bool};
use crate::dsl::error::DslError;
pub use crate::dsl::registered::REGISTERED_QUERIES;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{Value, bool_value, float_value, object_value, suggest, text_or_null};

/// `Operator`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    /// `OR`, the default.
    Or,
    /// `AND`.
    And,
}

/// A parsed query.
#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    /// `MatchAllQueryBuilder`.
    MatchAll {
        /// The boost.
        boost: f32,
    },
    /// `TermQueryBuilder`.
    Term {
        /// The field.
        field: String,
        /// The value.
        value: Value,
        /// The boost.
        boost: f32,
    },
    /// `MatchQueryBuilder`, of type boolean.
    Match {
        /// The field.
        field: String,
        /// The text (or other value) to match.
        query: Value,
        /// How the terms combine.
        operator: Operator,
        /// `minimum_should_match`.
        minimum_should_match: Option<String>,
        /// `zero_terms_query: all`.
        zero_terms_all: bool,
        /// The boost.
        boost: f32,
    },
    /// `RangeQueryBuilder`.
    Range {
        /// The field.
        field: String,
        /// The lower bound.
        from: Option<Value>,
        /// The upper bound.
        to: Option<Value>,
        /// Whether the lower bound is included.
        include_lower: bool,
        /// Whether the upper bound is included.
        include_upper: bool,
        /// The boost.
        boost: f32,
    },
    /// `BoolQueryBuilder`.
    Bool(Box<BoolQuery>),
}

/// `parseInnerQueryBuilder`: the parser is on (or just before) the query
/// object's start; afterwards it is on the query object's end.
pub fn parse_inner(p: &mut Parser<'_>) -> Result<Query, DslError> {
    if p.current_token() != Some(Kind::StartObject) && p.next_token() != Some(Kind::StartObject) {
        return Err(DslError::parsing(
            p.location(),
            "[_na] query malformed, must start with start_object",
        ));
    }
    if p.next_token() == Some(Kind::EndObject) {
        let at = p.location();
        return Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!(
                "query malformed, empty clause found at [{}:{}]",
                at.line, at.column
            ),
        ));
    }
    if p.current_token() != Some(Kind::FieldName) {
        return Err(DslError::parsing(
            p.location(),
            "[_na] query malformed, no field after start_object",
        ));
    }
    let name = p.text();
    if p.next_token() != Some(Kind::StartObject) {
        return Err(DslError::parsing(
            p.location(),
            // ALLOC: error path; one message.
            format!("[{name}] query malformed, no start_object after query name"),
        ));
    }
    let query = match name {
        "match_all" => parse_match_all(p)?,
        "term" => parse_term(p)?,
        "match" => parse_match(p)?,
        "range" => parse_range(p)?,
        // ALLOC: one box a bool query, nested at most as deep as the tokenizer allows.
        "bool" => Query::Bool(Box::new(parse_bool(p)?)),
        known if REGISTERED_QUERIES.contains(&known) => {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!("the [{known}] query")));
        }
        unknown => {
            let at = p.location();
            // ALLOC: error path; one message.
            let reason = format!(
                "unknown query [{unknown}]{}",
                suggest(unknown, REGISTERED_QUERIES)
            );
            let cause = crate::failure::Failure::plain(
                "named_object_not_found_exception",
                // ALLOC: error path; one message.
                format!("[{}:{}] unknown field [{unknown}]", at.line, at.column),
            );
            return Err(DslError::Failure(
                crate::dsl::error::parsing_failure(at, reason).caused_by(cause),
            ));
        }
    };
    if p.current_token() != Some(Kind::EndObject) {
        return Err(malformed(p, name));
    }
    if p.next_token() != Some(Kind::EndObject) {
        return Err(malformed(p, name));
    }
    Ok(query)
}

fn malformed(p: &Parser<'_>, name: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!(
            "[{name}] malformed query, expected [END_OBJECT] but found [{}]",
            p.current_token().map_or("null", Kind::name)
        ),
    )
}

fn does_not_support(p: &Parser<'_>, query: &str, field: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::parsing(
        p.location(),
        format!("[{query}] query does not support [{field}]"),
    )
}

fn multiple_fields(p: &Parser<'_>, query: &str, first: &str, second: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("[{query}] query doesn't support multiple fields, found [{first}] and [{second}]"),
    )
}

/// Advances past a value or a whole object or array.
fn next_or_end(p: &mut Parser<'_>, end: Kind) -> Option<Kind> {
    match p.next_token() {
        Some(k) if k == end => None,
        None => None,
        other => other,
    }
}

/// `MatchAllQueryBuilder.fromXContent` (an `ObjectParser` of `boost` and
/// `_name`).
fn parse_match_all(p: &mut Parser<'_>) -> Result<Query, DslError> {
    let mut boost = 1.0;
    let mut field = "";
    while let Some(token) = next_or_end(p, Kind::EndObject) {
        match token {
            Kind::FieldName => field = p.text(),
            _ if field == "boost" => boost = float_value(p)?,
            _ if field == "_name" => return Err(DslError::unsupported("[_name]")),
            _ => {
                return Err(DslError::xcontent(
                    p.location(),
                    // ALLOC: error path; one message.
                    &format!(
                        "[match_all] unknown field [{field}]{}",
                        suggest(field, &["boost", "_name"])
                    ),
                    None,
                ));
            }
        }
    }
    Ok(Query::MatchAll { boost })
}

/// `TermQueryBuilder.fromXContent`.
fn parse_term(p: &mut Parser<'_>) -> Result<Query, DslError> {
    let mut field: Option<&str> = None;
    let mut value = None;
    let mut boost = 1.0;
    let mut current = "";
    while let Some(token) = next_or_end(p, Kind::EndObject) {
        match token {
            Kind::FieldName => current = p.text(),
            Kind::StartObject => {
                if let Some(f) = field {
                    return Err(multiple_fields(p, "term", f, current));
                }
                field = Some(current);
                while let Some(token) = next_or_end(p, Kind::EndObject) {
                    match (token, current) {
                        (Kind::FieldName, _) => current = p.text(),
                        (_, "term" | "value") => value = object_value(p)?,
                        (_, "boost") => boost = float_value(p)?,
                        (_, "case_insensitive") => {
                            if bool_value(p)? {
                                return Err(DslError::unsupported("[case_insensitive] true"));
                            }
                        }
                        (_, "_name") => return Err(DslError::unsupported("[_name]")),
                        _ => return Err(does_not_support(p, "term", current)),
                    }
                }
            }
            Kind::StartArray => {
                return Err(DslError::parsing(
                    p.location(),
                    "[term] query does not support array of values",
                ));
            }
            _ => {
                if let Some(f) = field {
                    return Err(multiple_fields(
                        p,
                        "term",
                        f,
                        p.current_name().unwrap_or(""),
                    ));
                }
                field = Some(current);
                value = object_value(p)?;
            }
        }
    }
    let (Some(field), Some(value)) = (field, value) else {
        return Err(DslError::illegal(if field.is_none() {
            "field name is null or empty"
        } else {
            "value cannot be null"
        }));
    };
    Ok(Query::Term {
        // ALLOC: one field name a clause, bounded by the body's recording.
        field: field.to_owned(),
        value,
        boost,
    })
}

/// `MatchQueryBuilder.fromXContent`.
fn parse_match(p: &mut Parser<'_>) -> Result<Query, DslError> {
    let mut field: Option<&str> = None;
    let mut value = None;
    let mut boost = 1.0;
    let mut operator = Operator::Or;
    let mut minimum_should_match = None;
    let mut zero_terms_all = false;
    let mut current = "";
    while let Some(token) = next_or_end(p, Kind::EndObject) {
        match token {
            Kind::FieldName => current = p.text(),
            Kind::StartObject => {
                if let Some(f) = field {
                    return Err(multiple_fields(p, "match", f, current));
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
                            format!("[match] unknown token [{}] after [{current}]", token.name()),
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
                        "zero_terms_query" => match p.text().to_ascii_lowercase().as_str() {
                            "none" => zero_terms_all = false,
                            "all" => zero_terms_all = true,
                            other => {
                                return Err(DslError::parsing(
                                    p.location(),
                                    // ALLOC: error path; one message.
                                    format!("Unsupported zero_terms_query value [{other}]"),
                                ));
                            }
                        },
                        "analyzer" if p.text() == "standard" => {}
                        "analyzer"
                        | "fuzziness"
                        | "prefix_length"
                        | "max_expansions"
                        | "fuzzy_rewrite"
                        | "fuzzy_transpositions"
                        | "lenient"
                        | "cutoff_frequency"
                        | "auto_generate_synonyms_phrase_query"
                        | "_name" => {
                            // ALLOC: error path; one message.
                            return Err(DslError::unsupported(&format!("[match] [{current}]")));
                        }
                        _ => return Err(does_not_support(p, "match", current)),
                    }
                }
            }
            _ => {
                let name = p.current_name().unwrap_or("");
                if let Some(f) = field {
                    return Err(multiple_fields(p, "match", f, name));
                }
                field = Some(name);
                value = object_value(p)?;
            }
        }
    }
    let Some(query) = value else {
        return Err(DslError::parsing(
            p.location(),
            "No text specified for text query",
        ));
    };
    Ok(Query::Match {
        // ALLOC: one field name a clause, bounded by the body's recording.
        field: field.unwrap_or("").to_owned(),
        query,
        operator,
        minimum_should_match,
        zero_terms_all,
        boost,
    })
}

/// `Operator.fromString`.
fn parse_operator(text: &str) -> Result<Operator, DslError> {
    match text.to_ascii_uppercase().as_str() {
        "OR" => Ok(Operator::Or),
        "AND" => Ok(Operator::And),
        other => Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("No enum constant org.opensearch.index.query.Operator.{other}"),
        )),
    }
}

const INVALID_LOWER: &str = "invalid lower bound for [range] query";
const INVALID_UPPER: &str = "invalid upper bound for [range] query";

/// `RangeQueryBuilder.fromXContent`.
fn parse_range(p: &mut Parser<'_>) -> Result<Query, DslError> {
    let mut field: Option<&str> = None;
    let (mut from, mut to) = (None, None);
    let (mut include_lower, mut include_upper): (Option<bool>, Option<bool>) = (None, None);
    let mut boost = 1.0;
    let mut current = "";
    while let Some(token) = next_or_end(p, Kind::EndObject) {
        match token {
            Kind::FieldName => current = p.text(),
            Kind::StartObject => {
                if let Some(f) = field {
                    return Err(multiple_fields(p, "range", f, current));
                }
                field = Some(current);
                while let Some(token) = next_or_end(p, Kind::EndObject) {
                    if token == Kind::FieldName {
                        current = p.text();
                        continue;
                    }
                    let lower = |p: &Parser<'_>| DslError::parsing(p.location(), INVALID_LOWER);
                    let upper = |p: &Parser<'_>| DslError::parsing(p.location(), INVALID_UPPER);
                    match current {
                        "from" if from.is_some() => return Err(lower(p)),
                        "from" => from = object_value(p)?,
                        "to" if to.is_some() => return Err(upper(p)),
                        "to" => to = object_value(p)?,
                        "include_lower" if include_lower.is_some() => return Err(lower(p)),
                        "include_lower" => include_lower = Some(bool_value(p)?),
                        "include_upper" if include_upper.is_some() => return Err(upper(p)),
                        "include_upper" => include_upper = Some(bool_value(p)?),
                        "boost" => boost = float_value(p)?,
                        "gt" | "gte" if from.is_some() || include_lower.is_some() => {
                            return Err(lower(p));
                        }
                        "gt" | "gte" => {
                            from = object_value(p)?;
                            include_lower = Some(current == "gte");
                        }
                        "lt" | "lte" if to.is_some() || include_upper.is_some() => {
                            return Err(upper(p));
                        }
                        "lt" | "lte" => {
                            to = object_value(p)?;
                            include_upper = Some(current == "lte");
                        }
                        "time_zone" | "format" | "relation" | "_name" => {
                            // ALLOC: error path; one message.
                            return Err(DslError::unsupported(&format!("[range] [{current}]")));
                        }
                        _ => return Err(does_not_support(p, "range", current)),
                    }
                }
            }
            _ => return Err(does_not_support(p, "range", current)),
        }
    }
    Ok(Query::Range {
        // ALLOC: one field name a clause, bounded by the body's recording.
        field: field.unwrap_or("").to_owned(),
        from,
        to,
        include_lower: include_lower.unwrap_or(true),
        include_upper: include_upper.unwrap_or(true),
        boost,
    })
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
