//! `simple_query_string`: `SimpleQueryStringBuilder.fromXContent` at
//! OpenSearch 3.10.0, with the same messages. A parameter OpenSearch takes
//! that this reader does not serve yet is refused rather than ignored.

use crate::dsl::error::DslError;
use crate::dsl::queries::More;
use crate::dsl::query::{Operator, parse_operator};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, float_value, int_value, text_or_null};

/// `SimpleQueryStringFlag.ALL`.
pub const ALL_FLAGS: i32 = -1;

/// The parameters of a `simple_query_string` query.
#[derive(Clone, Debug, PartialEq)]
pub struct SimpleQueryString {
    /// The query text.
    pub query: String,
    /// The fields or patterns, each with its boost; empty for the default
    /// field (`*`).
    pub fields: Vec<(String, f32)>,
    /// The enabled operators, as `SimpleQueryStringFlag` bits.
    pub flags: i32,
    /// How the clauses of a query string combine.
    pub operator: Operator,
    /// `lenient`, when set.
    pub lenient: Option<bool>,
    /// `minimum_should_match`, applied when the result is a boolean query.
    pub minimum_should_match: Option<String>,
    /// `fuzzy_prefix_length`.
    pub fuzzy_prefix_length: i32,
    /// `fuzzy_max_expansions`.
    pub fuzzy_max_expansions: i32,
    /// `fuzzy_transpositions`.
    pub fuzzy_transpositions: bool,
}

fn refused(param: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::unsupported(&format!("[simple_query_string] [{param}]"))
}

/// `SimpleQueryStringFlag.resolveFlags`.
fn resolve_flags(text: &str) -> Result<i32, DslError> {
    if text.is_empty() {
        return Ok(ALL_FLAGS);
    }
    let mut magic = 0;
    for s in text.split('|') {
        if s.is_empty() {
            continue;
        }
        magic |= match s.to_ascii_uppercase().as_str() {
            "NONE" => return Ok(0),
            "ALL" => return Ok(-1),
            "AND" => 1,
            "NOT" => 1 << 1,
            "OR" => 1 << 2,
            "PREFIX" => 1 << 3,
            "PHRASE" => 1 << 4,
            "PRECEDENCE" => 1 << 5,
            "ESCAPE" => 1 << 6,
            "WHITESPACE" => 1 << 7,
            "FUZZY" => 1 << 8,
            "NEAR" | "SLOP" => 1 << 9,
            _ => {
                // ALLOC: error path; one message.
                return Err(DslError::illegal(format!(
                    "Unknown simple_query_string flag [{s}]"
                )));
            }
        };
    }
    Ok(magic)
}

/// `Float.parseFloat` of a field's boost.
fn field_boost(text: &str) -> Result<f32, DslError> {
    let trimmed = text.trim();
    trimmed.parse::<f32>().map_err(|_| {
        DslError::Failure(crate::failure::Failure::plain(
            "number_format_exception",
            if trimmed.is_empty() {
                // ALLOC: error path; one message.
                "empty String".to_owned()
            } else {
                // ALLOC: error path; one message.
                format!("For input string: \"{trimmed}\"")
            },
        ))
    })
}

/// `QueryParserHelper.parseFieldsAndWeights` of one entry.
fn push_field(p: &Parser<'_>, fields: &mut Vec<(String, f32)>) -> Result<(), DslError> {
    let text = p.text();
    let (name, boost) = match text.split_once('^') {
        Some((name, boost)) => (name, field_boost(boost)?),
        None => (text, 1.0),
    };
    if boost < 0.0 {
        return Err(DslError::unsupported(
            "a [simple_query_string] field with a negative boost",
        ));
    }
    if name.is_empty() {
        return Err(DslError::unsupported(
            "a [simple_query_string] field with an empty name",
        ));
    }
    if fields.len() >= super::full_text::MAX_LISTED_FIELDS {
        // ALLOC: error path; one message.
        return Err(DslError::unsupported(&format!(
            "a [simple_query_string] listing more than [{}] fields",
            super::full_text::MAX_LISTED_FIELDS
        )));
    }
    // ALLOC: one name a listed field, at most MAX_LISTED_FIELDS.
    fields.push((name.to_owned(), boost));
    Ok(())
}

/// `SimpleQueryStringBuilder.fromXContent`: the parser is on the query
/// object's start, and ends on its end. Returns the query and its boost.
pub fn parse_simple_query_string(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut q = SimpleQueryString {
        // ALLOC: the query text, once, bounded by the body.
        query: String::new(),
        // ALLOC: one entry a listed field, at most MAX_LISTED_FIELDS.
        fields: Vec::new(),
        flags: ALL_FLAGS,
        operator: Operator::Or,
        lenient: None,
        minimum_should_match: None,
        fuzzy_prefix_length: 0,
        fuzzy_max_expansions: 50,
        fuzzy_transpositions: true,
    };
    let mut text: Option<String> = None;
    let mut boost = 1.0;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartArray) if current == "fields" => {
                // ALLOC: the previous list, if any, is dropped.
                q.fields = Vec::new();
                loop {
                    match p.next_token() {
                        Some(Kind::EndArray) | None => break,
                        Some(_) => push_field(p, &mut q.fields)?,
                    }
                }
            }
            Some(Kind::StartArray) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!("[simple_query_string] query does not support [{current}]"),
                ));
            }
            Some(k) if k.is_value() => match current {
                // ALLOC: one string a query, bounded by the body's recording.
                "query" => text = text_or_null(p).map(str::to_owned),
                "boost" => boost = float_value(p)?,
                "default_operator" => q.operator = parse_operator(p.text())?,
                "flags" => {
                    q.flags = if k == Kind::Number {
                        match int_value(p)? {
                            n if n < 0 => ALL_FLAGS,
                            n => n,
                        }
                    } else {
                        resolve_flags(p.text())?
                    };
                }
                "lenient" => q.lenient = Some(bool_value(p)?),
                "analyze_wildcard" => {
                    if bool_value(p)? {
                        return Err(refused(current));
                    }
                }
                "analyzer" if p.text() == "standard" => {}
                // ALLOC: one string a query, bounded by the body's recording.
                "minimum_should_match" => {
                    q.minimum_should_match = text_or_null(p).map(str::to_owned);
                }
                "quote_field_suffix" => {
                    if text_or_null(p).is_some() {
                        return Err(refused(current));
                    }
                }
                "all_fields" | "locale" | "lowercase_expanded_terms" => {}
                "auto_generate_synonyms_phrase_query" => {
                    bool_value(p)?;
                }
                "fuzzy_prefix_length" => q.fuzzy_prefix_length = int_value(p)?,
                "fuzzy_max_expansions" => q.fuzzy_max_expansions = int_value(p)?,
                "fuzzy_transpositions" => q.fuzzy_transpositions = bool_value(p)?,
                "analyzer" | "_name" => return Err(refused(current)),
                _ => {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!("[simple_query_string] unsupported field [{current}]"),
                    ));
                }
            },
            Some(k) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "[simple_query_string] unknown token [{}] after [{current}]",
                        k.name()
                    ),
                ));
            }
        }
    }
    let Some(text) = text else {
        return Err(DslError::parsing(
            p.location(),
            "[simple_query_string] query text missing",
        ));
    };
    if boost < 0.0 {
        return Err(DslError::unsupported(
            "a [simple_query_string] with a negative boost",
        ));
    }
    q.query = text;
    // ALLOC: one box a simple_query_string query.
    Ok(More::SimpleQueryString(Box::new(q), boost))
}
