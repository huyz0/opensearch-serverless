//! `MatchQueryBuilder.fromXContent`: the `match` query's body, with its
//! `fuzziness` options and the refusals of the rest.

use crate::dsl::error::DslError;
use crate::dsl::query::{
    Operator, Query, does_not_support, multiple_fields, next_or_end, parse_operator,
};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, float_value, int_value, object_value, text_or_null};

/// `MatchQueryBuilder.fromXContent`.
pub(crate) fn parse_match(p: &mut Parser<'_>) -> Result<Query, DslError> {
    let mut field: Option<&str> = None;
    let mut value = None;
    let mut boost = 1.0;
    let mut operator = Operator::Or;
    let mut minimum_should_match = None;
    let mut zero_terms_all = false;
    let mut fuzziness = None;
    let (mut prefix_length, mut max_expansions, mut transpositions) = (0, 50, true);
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
                        "fuzziness" => {
                            fuzziness = Some(crate::dsl::queries::term_level::parse_fuzziness(p)?);
                        }
                        "prefix_length" => prefix_length = int_value(p)?,
                        "max_expansions" => max_expansions = int_value(p)?,
                        "fuzzy_transpositions" => transpositions = bool_value(p)?,
                        "analyzer"
                        | "fuzzy_rewrite"
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
    if let Some(fuzziness) = fuzziness {
        let m = crate::dsl::queries::full_text::FuzzyMatch {
            // ALLOC: one field name a clause, bounded by the body's recording.
            field: field.unwrap_or("").to_owned(),
            query,
            operator,
            minimum_should_match,
            zero_terms_all,
            fuzziness,
            prefix_length,
            max_expansions,
            transpositions,
        };
        // ALLOC: one box a fuzzy match, bounded by the body's recording.
        return Ok(Query::More(Box::new(
            crate::dsl::queries::More::FuzzyMatch(Box::new(m), boost),
        )));
    }
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
