//! The query builders of the subset and their parsers: `match_all`,
//! `term`, `match`, `range` and `bool`.
//!
//! Each parser is ported from its builder's `fromXContent` at OpenSearch
//! 3.10.0 (server/src/main/java/org/opensearch/index/query/), with the same
//! messages and locations; `parse_inner` is
//! `AbstractQueryBuilder.parseInnerQueryBuilder`. A parameter OpenSearch
//! takes but this reader does not serve yet is refused
//! ([`DslError::unsupported`]) rather than ignored.

use crate::date::{DateFormat, DateSpec};
use crate::date_zone::parse_time_zone;
use crate::dsl::bool_query::{BoolQuery, parse_bool};
use crate::dsl::error::DslError;
pub use crate::dsl::registered::REGISTERED_QUERIES;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{Value, bool_value, float_value, name_value, object_value, suggest};

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
        /// The `format` and `time_zone` the bounds are read in.
        date: DateSpec,
    },
    /// `BoolQueryBuilder`.
    Bool(Box<BoolQuery>),
    /// A query of [`crate::dsl::queries`].
    More(Box<crate::dsl::queries::More>),
    /// A query that carries a `_name`.
    Named(Box<Named>),
}

/// A query and the `_name` it was given.
#[derive(Clone, Debug, PartialEq)]
pub struct Named {
    /// The name.
    pub name: String,
    /// The query it names.
    pub query: Query,
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
    // A parent's `_name` may be read before its clauses; keep it out of theirs.
    let outer = p.take_name();
    let query = match name {
        "match_all" => parse_match_all(p)?,
        "term" => parse_term(p)?,
        "match" => crate::dsl::match_parse::parse_match(p)?,
        "range" => parse_range(p)?,
        // ALLOC: one box a bool query, nested at most as deep as the tokenizer allows.
        "bool" => Query::Bool(Box::new(parse_bool(p)?)),
        // ALLOC: one box a script query, nested at most as deep as the tokenizer allows.
        "script" if crate::painless_switch::native_compile() => {
            Query::More(Box::new(crate::dsl::queries::script_query::parse(p)?))
        }
        // ALLOC: one box a query of the module, nested at most as deep as the tokenizer allows.
        more if crate::dsl::queries::NAMES.contains(&more) => {
            Query::More(Box::new(crate::dsl::queries::parse(more, p)?))
        }
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
    let mine = p.take_name();
    if let Some(outer) = outer {
        p.set_name(outer);
    }
    Ok(match mine {
        // ALLOC: one box a named query, at most 64 a search.
        Some(name) => Query::Named(Box::new(Named { name, query })),
        None => query,
    })
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

pub(crate) fn does_not_support(p: &Parser<'_>, query: &str, field: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::parsing(
        p.location(),
        format!("[{query}] query does not support [{field}]"),
    )
}

pub(crate) fn multiple_fields(p: &Parser<'_>, query: &str, first: &str, second: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("[{query}] query doesn't support multiple fields, found [{first}] and [{second}]"),
    )
}

/// Advances past a value or a whole object or array.
pub(crate) fn next_or_end(p: &mut Parser<'_>, end: Kind) -> Option<Kind> {
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
            _ if field == "_name" => name_value(p)?,
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
    let mut ci = false;
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
                        (_, "case_insensitive") => ci = bool_value(p)?,
                        (_, "_name") => name_value(p)?,
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
    if ci {
        // ALLOC: one box a clause, bounded by the body's recording.
        return Ok(Query::More(Box::new(
            crate::dsl::queries::More::CaseInsensitive {
                kind: crate::dsl::queries::case_insensitive::Kind::Term,
                // ALLOC: one field name and one value a clause, bounded by the body's recording.
                field: field.to_owned(),
                value: value.text().to_owned(),
                boost,
            },
        )));
    }
    Ok(Query::Term {
        // ALLOC: one field name a clause, bounded by the body's recording.
        field: field.to_owned(),
        value,
        boost,
    })
}

/// `Operator.fromString`.
pub(crate) fn parse_operator(text: &str) -> Result<Operator, DslError> {
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
    let mut date = DateSpec::default();
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
                        "time_zone" => date = range_zone(p, date)?,
                        "format" => date = range_format(p, date)?,
                        "_name" => name_value(p)?,
                        "relation" => {
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
        date,
    })
}

/// A range's `time_zone`: a fixed zone, which only a date field takes.
fn range_zone(p: &Parser<'_>, date: DateSpec) -> Result<DateSpec, DslError> {
    match (p.current_token(), parse_time_zone(p.text())) {
        (Some(Kind::String), Some(zone)) => Ok(DateSpec {
            zone: zone.fixed,
            region: zone.region,
            named: true,
            ..date
        }),
        // ALLOC: error path; one message.
        _ => Err(DslError::unsupported(&format!(
            "[range] [time_zone] [{}]",
            p.text()
        ))),
    }
}

/// A range's `format`: one of the formats this node reads, which only a date field takes.
fn range_format(p: &Parser<'_>, date: DateSpec) -> Result<DateSpec, DslError> {
    match (p.current_token(), DateFormat::of(p.text())) {
        (Some(Kind::String), Some(format)) => Ok(DateSpec {
            format,
            named: true,
            ..date
        }),
        // ALLOC: error path; one message.
        _ => Err(DslError::unsupported(&format!(
            "[range] [format] [{}]",
            p.text()
        ))),
    }
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
