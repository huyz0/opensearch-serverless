//! The full-text parsers: `match_phrase` and `multi_match`, each its
//! builder's `fromXContent` and constructor checks at OpenSearch 3.10.0.

use crate::dsl::error::DslError;
use crate::dsl::queries::More;
use crate::dsl::queries::single_field::{self, does_not_support};
use crate::dsl::query::Operator;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{Value, bool_value, float_value, int_value, object_value, text_or_null};

/// The most entries one `multi_match`'s `fields` may hold on this node,
/// names repeated included. OpenSearch has no such bound: it bounds the
/// fields they resolve to (at the clause limit), which this node checks too.
pub const MAX_LISTED_FIELDS: usize = 4 * crate::dsl::bool_query::MAX_CLAUSES;

/// `MultiMatchQueryBuilder.Type`, the kinds served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiType {
    /// `best_fields` (or `boolean`): the best field's match, plus the
    /// others' times the tie breaker (0 by default).
    BestFields,
    /// `most_fields`: every field's match summed (a tie breaker of 1).
    MostFields,
    /// `phrase`: each field's phrase, as `best_fields` combines them.
    Phrase,
}

impl MultiType {
    /// `Type.tieBreaker()`.
    pub fn tie_breaker(self) -> f32 {
        match self {
            MultiType::MostFields => 1.0,
            MultiType::BestFields | MultiType::Phrase => 0.0,
        }
    }
}

/// `zero_terms_query`: true for `all`.
fn zero_terms(p: &Parser<'_>) -> Result<bool, DslError> {
    match p.text() {
        t if t.eq_ignore_ascii_case("none") => Ok(false),
        t if t.eq_ignore_ascii_case("all") => Ok(true),
        other => Err(DslError::parsing(
            p.location(),
            // ALLOC: error path; one message.
            format!("Unsupported zero_terms_query value [{other}]"),
        )),
    }
}

fn unsupported(query: &str, param: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::unsupported(&format!("[{query}] [{param}]"))
}

/// `MatchPhraseQueryBuilder.fromXContent`.
pub fn parse_match_phrase(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut value: Option<Value> = None;
    let mut boost = 1.0;
    let mut slop = 0;
    let mut zero_terms_all = false;
    let field = single_field::parse(p, "match_phrase", |p, param| {
        match param {
            None | Some("query") => value = object_value(p)?,
            Some("boost") => boost = float_value(p)?,
            Some("slop") => slop = int_value(p)?,
            Some("zero_terms_query") => zero_terms_all = zero_terms(p)?,
            Some("analyzer") if p.text() == "standard" => {}
            Some(param @ ("analyzer" | "_name")) => return Err(unsupported("match_phrase", param)),
            Some(param) => return Err(does_not_support(p, "match_phrase", param)),
        }
        Ok(())
    })?;
    let Some(field) = field.filter(|f| !f.is_empty()) else {
        return Err(DslError::illegal("[match_phrase] requires fieldName"));
    };
    let Some(query) = value else {
        return Err(DslError::illegal("[match_phrase] requires query value"));
    };
    let Ok(slop) = u32::try_from(slop) else {
        return Err(DslError::illegal("No negative slop allowed."));
    };
    Ok(More::MatchPhrase {
        // ALLOC: one field name a clause, bounded by the body's recording.
        field: field.to_owned(),
        query,
        slop,
        zero_terms_all,
        boost,
    })
}

/// `MultiMatchQueryBuilder.parseFieldAndBoost`: `name^boost`. A name given
/// again is kept twice here; [`crate::dsl::queries::full_text_build`] keeps
/// the later boost, as `fieldsBoosts.put` does.
fn field_and_boost(p: &Parser<'_>, fields: &mut Vec<(String, f32)>) -> Result<(), DslError> {
    let text = p.text();
    let (name, boost) = match text.split_once('^') {
        Some((name, boost)) => (name, parse_float(boost)?),
        None => (text, 1.0),
    };
    if fields.len() >= MAX_LISTED_FIELDS {
        return Err(DslError::unsupported(
            // ALLOC: error path; one message.
            &format!("a [multi_match] listing more than [{MAX_LISTED_FIELDS}] fields"),
        ));
    }
    // ALLOC: one name a listed field, at most MAX_LISTED_FIELDS.
    fields.push((name.to_owned(), boost));
    Ok(())
}

/// `Float.parseFloat`: surrounding whitespace ignored.
fn parse_float(text: &str) -> Result<f32, DslError> {
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

/// `MultiMatchQueryBuilder.Type.parse`.
fn multi_type(text: &str) -> Result<MultiType, DslError> {
    match text {
        "best_fields" | "boolean" => Ok(MultiType::BestFields),
        "most_fields" => Ok(MultiType::MostFields),
        "phrase" => Ok(MultiType::Phrase),
        "cross_fields" | "phrase_prefix" | "bool_prefix" => {
            // ALLOC: error path; one message.
            Err(DslError::unsupported(&format!(
                "[multi_match] type [{text}]"
            )))
        }
        other => Err(DslError::Failure(crate::failure::Failure::opensearch(
            "parse_exception",
            // ALLOC: error path; one message.
            format!("failed to parse [multi_match] query type [{other}]. unknown type."),
        ))),
    }
}

/// The parameters of a `multi_match` query.
#[derive(Clone, Debug, PartialEq)]
pub struct MultiMatch {
    /// The text.
    pub query: Value,
    /// The fields or patterns, each with its boost, in the order given.
    pub fields: Vec<(String, f32)>,
    /// How the fields' queries combine.
    pub kind: MultiType,
    /// How each field's terms combine.
    pub operator: Operator,
    /// `minimum_should_match`, applied to each field's query.
    pub minimum_should_match: Option<String>,
    /// The tie breaker; the type's when absent.
    pub tie_breaker: Option<f32>,
    /// The phrase slop.
    pub slop: u32,
    /// `zero_terms_query: all`.
    pub zero_terms_all: bool,
    /// `lenient`: a field whose type cannot take the text matches nothing.
    pub lenient: Option<bool>,
}

/// `MultiMatchQueryBuilder.fromXContent`.
pub fn parse_multi_match(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut m = MultiMatch {
        query: Value::Bool(false),
        // ALLOC: one entry a listed field, at most MAX_LISTED_FIELDS.
        fields: Vec::new(),
        kind: MultiType::BestFields,
        operator: Operator::Or,
        minimum_should_match: None,
        tie_breaker: None,
        slop: 0,
        zero_terms_all: false,
        lenient: None,
    };
    let mut value = None;
    let mut slop = 0;
    let mut boost = 1.0;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartArray) if current == "fields" => loop {
                match p.next_token() {
                    Some(Kind::EndArray) | None => break,
                    Some(k) if k.is_value() => field_and_boost(p, &mut m.fields)?,
                    Some(_) => return Err(does_not_support(p, "multi_match", current)),
                }
            },
            Some(k) if k.is_value() && current == "fields" => field_and_boost(p, &mut m.fields)?,
            Some(Kind::StartObject) if current == "fields" => {
                return Err(does_not_support(p, "multi_match", current));
            }
            Some(k) if k.is_value() => match current {
                "query" => value = object_value(p)?,
                "type" => m.kind = multi_type(p.text())?,
                "boost" => boost = float_value(p)?,
                "slop" => slop = int_value(p)?,
                "operator" => m.operator = crate::dsl::query::parse_operator(p.text())?,
                // ALLOC: one string a clause, bounded by the body's recording.
                "minimum_should_match" => {
                    m.minimum_should_match = text_or_null(p).map(str::to_owned)
                }
                "tie_breaker" => m.tie_breaker = Some(float_value(p)?),
                "lenient" => m.lenient = Some(bool_value(p)?),
                "zero_terms_query" => m.zero_terms_all = zero_terms(p)?,
                "analyzer" if p.text() == "standard" => {}
                "auto_generate_synonyms_phrase_query" | "fuzzy_transpositions" => {
                    bool_value(p)?;
                }
                "analyzer" | "fuzziness" | "prefix_length" | "max_expansions" | "fuzzy_rewrite"
                | "cutoff_frequency" | "_name" => return Err(unsupported("multi_match", current)),
                _ => return Err(does_not_support(p, "multi_match", current)),
            },
            Some(k) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "[multi_match] unknown token [{}] after [{current}]",
                        k.name()
                    ),
                ));
            }
        }
    }
    let Some(query) = value else {
        return Err(DslError::parsing(
            p.location(),
            "No text specified for multi_match query",
        ));
    };
    if m.fields.is_empty() {
        return Err(DslError::unsupported(
            "a [multi_match] without [fields] (index.query.default_field)",
        ));
    }
    let Ok(slop) = u32::try_from(slop) else {
        return Err(DslError::illegal("No negative slop allowed."));
    };
    m.query = query;
    m.slop = slop;
    // ALLOC: one box a multi_match query.
    Ok(More::MultiMatch(Box::new(m), boost))
}
