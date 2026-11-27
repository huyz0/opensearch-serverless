//! `query_string`: `QueryStringQueryBuilder.fromXContent` at OpenSearch
//! 3.10.0, with the same messages. A parameter OpenSearch takes that this
//! reader does not serve yet is refused rather than ignored.

use super::full_text::MultiType;
use super::simple_string::push_field;
use super::{Fuzziness, More, term_level};
use crate::dsl::error::DslError;
use crate::dsl::query::{Operator, parse_operator};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, float_value, int_value, text_or_null};

/// The parameters of a `query_string` query.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryString {
    /// The query text.
    pub query: String,
    /// `default_field`.
    pub default_field: Option<String>,
    /// `fields`, each with its boost; empty when not given.
    pub fields: Vec<(String, f32)>,
    /// How the clauses of a query string combine.
    pub operator: Operator,
    /// `type`: how the queries of several fields combine.
    pub kind: MultiType,
    /// `tie_breaker`; the type's when absent.
    pub tie_breaker: Option<f32>,
    /// `phrase_slop`.
    pub phrase_slop: i32,
    /// `allow_leading_wildcard`; true when absent, the index setting's
    /// default.
    pub allow_leading_wildcard: Option<bool>,
    /// `lenient`, when set.
    pub lenient: Option<bool>,
    /// `minimum_should_match`, applied when the result is a boolean query.
    pub minimum_should_match: Option<String>,
    /// `fuzziness`.
    pub fuzziness: Fuzziness,
    /// `fuzzy_prefix_length`.
    pub fuzzy_prefix_length: i32,
    /// `fuzzy_max_expansions`.
    pub fuzzy_max_expansions: i32,
    /// `fuzzy_transpositions`.
    pub fuzzy_transpositions: bool,
    /// `escape`: the text is taken literally.
    pub escape: bool,
}

fn refused(param: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::unsupported(&format!("[query_string] [{param}]"))
}

/// `QueryStringQueryBuilder.fromXContent`. The parser is on the query
/// object's start, and ends on its end.
pub fn parse_query_string(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut q = QueryString {
        // ALLOC: empty; the query text is one string, bounded by the body.
        query: String::new(),
        default_field: None,
        // ALLOC: one entry a listed field, at most MAX_LISTED_FIELDS.
        fields: Vec::new(),
        operator: Operator::Or,
        kind: MultiType::BestFields,
        tie_breaker: None,
        phrase_slop: 0,
        allow_leading_wildcard: None,
        lenient: None,
        minimum_should_match: None,
        fuzziness: Fuzziness::AUTO,
        fuzzy_prefix_length: 0,
        fuzzy_max_expansions: 50,
        fuzzy_transpositions: true,
        escape: false,
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
                        Some(Kind::Null) => {
                            return Err(DslError::parsing(
                                p.location(),
                                // ALLOC: error path; one message.
                                format!("[query_string] field name in [{current}] cannot be null"),
                            ));
                        }
                        Some(_) => push_field(p, &mut q.fields)?,
                    }
                }
            }
            Some(Kind::StartArray) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!("[query_string] query does not support [{current}]"),
                ));
            }
            Some(k) if k.is_value() => match current {
                // ALLOC: one string a query, bounded by the body.
                "query" => text = Some(p.text().to_owned()),
                // ALLOC: one string a field name.
                "default_field" => q.default_field = Some(p.text().to_owned()),
                "default_operator" => q.operator = parse_operator(p.text())?,
                "allow_leading_wildcard" => q.allow_leading_wildcard = Some(bool_value(p)?),
                "enable_position_increments" => {
                    if !bool_value(p)? {
                        return Err(refused(current));
                    }
                }
                "escape" => q.escape = bool_value(p)?,
                "fuzzy_prefix_length" => q.fuzzy_prefix_length = int_value(p)?,
                "fuzzy_max_expansions" => q.fuzzy_max_expansions = int_value(p)?,
                "phrase_slop" => q.phrase_slop = int_value(p)?,
                "fuzziness" => q.fuzziness = term_level::parse_fuzziness(p)?,
                "boost" => boost = float_value(p)?,
                "type" => {
                    let t = p.text();
                    q.kind = match t {
                        "cross_fields" | "phrase_prefix" | "bool_prefix" => {
                            // ALLOC: error path; one message.
                            return Err(DslError::unsupported(&format!(
                                "[query_string] type [{t}]"
                            )));
                        }
                        other => super::full_text::multi_type(other)?,
                    };
                }
                "tie_breaker" => q.tie_breaker = Some(float_value(p)?),
                "analyze_wildcard" => {
                    if bool_value(p)? {
                        return Err(refused(current));
                    }
                }
                "minimum_should_match" => {
                    // ALLOC: one string a minimum_should_match.
                    q.minimum_should_match = text_or_null(p).map(str::to_owned);
                }
                "lenient" => q.lenient = Some(bool_value(p)?),
                "max_determinized_states" => {
                    int_value(p)?;
                }
                "auto_generate_synonyms_phrase_query" => {
                    bool_value(p)?;
                }
                "fuzzy_transpositions" => q.fuzzy_transpositions = bool_value(p)?,
                "analyzer" if p.text() == "standard" => {}
                "quote_field_suffix" | "rewrite" | "fuzzy_rewrite" => {
                    if text_or_null(p).is_some() {
                        return Err(refused(current));
                    }
                }
                "_name" => crate::dsl::values::name_value(p)?,
                "analyzer" | "quote_analyzer" | "time_zone" => {
                    return Err(refused(current));
                }
                _ => {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!("[query_string] query does not support [{current}]"),
                    ));
                }
            },
            Some(k) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "[query_string] unknown token [{}] after [{current}]",
                        k.name()
                    ),
                ));
            }
        }
    }
    let Some(text) = text else {
        return Err(DslError::parsing(
            p.location(),
            "[query_string] must be provided with a [query]",
        ));
    };
    if boost < 0.0 {
        return Err(DslError::unsupported(
            "a [query_string] with a negative boost",
        ));
    }
    q.query = text;
    // ALLOC: one box a query_string query.
    Ok(More::QueryString(Box::new(q), boost))
}
