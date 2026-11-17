//! The term-level parsers: `terms`, `prefix`, `wildcard`, `regexp`,
//! `fuzzy`, `exists` and `ids`, each its builder's `fromXContent` and
//! constructor checks at OpenSearch 3.10.0.

use crate::dsl::error::DslError;
use crate::dsl::queries::single_field::{self, does_not_support};
use crate::dsl::queries::{Fuzziness, More};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{Value, bool_value, float_value, int_value, object_value, text_or_null};

/// `index.max_terms_count`'s default, the most values one `terms` or `ids`
/// query may hold, is never reached: a body's recording
/// ([`crate::dsl::MAX_RECORDED_BYTES`]) holds fewer values than that.
/// `RegexpQueryBuilder.DEFAULT_DETERMINIZE_WORK_LIMIT`.
pub const DEFAULT_DETERMINIZE_WORK_LIMIT: i32 = 10_000;

fn owned(s: &str) -> String {
    // ALLOC: one field name or value a clause, bounded by the body's recording.
    s.to_owned()
}

/// The checks the string builders' constructors make.
fn field_and_value(
    field: Option<&str>,
    value: Option<String>,
) -> Result<(String, String), DslError> {
    let field = field.filter(|f| !f.is_empty());
    let Some(field) = field else {
        return Err(DslError::illegal("field name is null or empty"));
    };
    let Some(value) = value else {
        return Err(DslError::illegal("value cannot be null"));
    };
    Ok((owned(field), value))
}

/// `case_insensitive: true` and `rewrite` are refused; `_name` too.
fn common(p: &Parser<'_>, query: &str, param: &str, boost: &mut f32) -> Result<bool, DslError> {
    match param {
        "boost" => *boost = float_value(p)?,
        "case_insensitive" => {
            if bool_value(p)? {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "[{query}] [case_insensitive] true"
                )));
            }
        }
        "rewrite" if p.current_token() == Some(Kind::Null) => {}
        "rewrite" | "_name" => {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!("[{query}] [{param}]")));
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// `TermsQueryBuilder.fromXContent` and `parseValues`.
pub fn parse_terms(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut field: Option<&str> = None;
    // ALLOC: none until a value is read; bounded by the body's recording.
    let mut values = Vec::new();
    let mut boost = 1.0;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartArray) => {
                if field.is_some() {
                    return Err(DslError::parsing(
                        p.location(),
                        "[terms] query does not support multiple fields",
                    ));
                }
                field = Some(current);
                loop {
                    match p.next_token() {
                        Some(Kind::EndArray) | None => break,
                        Some(k) if k.is_value() => {
                            let Some(v) = object_value(p)? else {
                                return Err(DslError::parsing(
                                    p.location(),
                                    "No value specified for terms query",
                                ));
                            };
                            values.push(v);
                        }
                        Some(_) => {
                            return Err(DslError::unsupported(
                                "a [terms] value that is an object or array",
                            ));
                        }
                    }
                }
            }
            Some(Kind::StartObject) => {
                if let Some(f) = field {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!(
                            "[terms] query does not support more than one field. Already got: \
                             [{f}] but also found [{current}]"
                        ),
                    ));
                }
                return Err(DslError::unsupported("the [terms] lookup"));
            }
            Some(k) if k.is_value() => match current {
                "boost" => boost = float_value(p)?,
                "value_type" if p.text() == "default" => {}
                "value_type" | "_name" => {
                    // ALLOC: error path; one message.
                    return Err(DslError::unsupported(&format!("[terms] [{current}]")));
                }
                _ => return Err(does_not_support(p, "terms", current)),
            },
            Some(k) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!("[terms] unknown token [{}] after [{current}]", k.name()),
                ));
            }
        }
    }
    let Some(field) = field else {
        return Err(DslError::parsing(
            p.location(),
            "[terms] query requires a field name, followed by array of terms or a document \
             lookup specification",
        ));
    };
    Ok(More::Terms {
        field: owned(field),
        values,
        boost,
    })
}

/// `PrefixQueryBuilder.fromXContent`.
pub fn parse_prefix(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut value = None;
    let mut boost = 1.0;
    let field = single_field::parse(p, "prefix", |p, param| {
        match param {
            None | Some("value") => value = text_or_null(p).map(owned),
            Some(param) => {
                if !common(p, "prefix", param, &mut boost)? {
                    return Err(does_not_support(p, "prefix", param));
                }
            }
        }
        Ok(())
    })?;
    let (field, value) = field_and_value(field, value)?;
    Ok(More::Prefix {
        field,
        value,
        boost,
    })
}

/// `WildcardQueryBuilder.fromXContent`.
pub fn parse_wildcard(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut value = None;
    let mut boost = 1.0;
    let field = single_field::parse(p, "wildcard", |p, param| {
        match param {
            None | Some("wildcard" | "value") => value = Some(owned(p.text())),
            Some(param) => {
                if !common(p, "wildcard", param, &mut boost)? {
                    return Err(does_not_support(p, "wildcard", param));
                }
            }
        }
        Ok(())
    })?;
    let (field, value) = field_and_value(field, value)?;
    Ok(More::Wildcard {
        field,
        value,
        boost,
    })
}

/// `RegexpQueryBuilder.fromXContent`, with the default flags only.
pub fn parse_regexp(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut value = None;
    let mut boost = 1.0;
    let mut max_states = DEFAULT_DETERMINIZE_WORK_LIMIT;
    let field = single_field::parse(p, "regexp", |p, param| {
        match param {
            None | Some("value") => value = text_or_null(p).map(owned),
            Some("max_determinized_states") => max_states = int_value(p)?,
            Some("flags") if matches!(text_or_null(p), None | Some("ALL")) => {}
            Some("flags_value") if int_value(p)? == 0xffff => {}
            Some(param @ ("flags" | "flags_value")) => {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "[regexp] [{param}] other than ALL"
                )));
            }
            Some(param) => {
                if !common(p, "regexp", param, &mut boost)? {
                    return Err(does_not_support(p, "regexp", param));
                }
            }
        }
        Ok(())
    })?;
    let (field, value) = field_and_value(field, value)?;
    if max_states < 0 {
        return Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("[regexp] max_determinized_states cannot be negative but was [{max_states}]"),
        ));
    }
    Ok(More::Regexp {
        field,
        value,
        boost,
    })
}

/// `Fuzziness.parse`.
fn parse_fuzziness(p: &Parser<'_>) -> Result<Fuzziness, DslError> {
    if !matches!(p.current_token(), Some(Kind::String | Kind::Number)) {
        return Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!(
                "Can't parse fuzziness on token: [{}]",
                p.current_token().map_or("null", Kind::name)
            ),
        ));
    }
    let text = p.text();
    let invalid = || {
        // ALLOC: error path; one message.
        DslError::illegal(format!("Invalid fuzziness value: {text}"))
    };
    if text.eq_ignore_ascii_case("AUTO") {
        return Ok(Fuzziness::AUTO);
    }
    if text.len() > 5
        && text
            .get(..5)
            .is_some_and(|s| s.eq_ignore_ascii_case("AUTO:"))
    {
        let mut parts = text.get(5..).unwrap_or("").split(',');
        let (Some(low), Some(high), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(DslError::Failure(crate::failure::Failure::opensearch(
                "parse_exception",
                "failed to find low and high distance values",
            )));
        };
        return match (low.parse::<i32>(), high.parse::<i32>()) {
            (Ok(low), Ok(high)) => Ok(Fuzziness::Auto { low, high }),
            _ => Err(DslError::Failure(crate::failure::Failure::opensearch(
                "parse_exception",
                // ALLOC: error path; one message.
                format!("failed to parse [{text}] as a \"auto:int,int\""),
            ))),
        };
    }
    if let Ok(n) = text.parse::<i32>() {
        if n < 0 {
            return Err(invalid());
        }
        return Ok(Fuzziness::Edits(u8::try_from(n.min(2)).unwrap_or(2)));
    }
    match text.parse::<f32>() {
        Ok(f) if f >= 0.0 && f.is_finite() => {
            // A fraction truncates toward zero, as `(int) asFloat()` does.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let edits = f.min(2.0) as u8;
            Ok(Fuzziness::Edits(edits))
        }
        _ => Err(invalid()),
    }
}

/// `FuzzyQueryBuilder.fromXContent` and its constructor's checks.
pub fn parse_fuzzy(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut value: Option<Value> = None;
    let mut boost = 1.0;
    let mut fuzziness = Fuzziness::AUTO;
    let mut prefix_length = 0;
    let mut max_expansions = 50;
    let mut transpositions = true;
    let field = single_field::parse(p, "fuzzy", |p, param| {
        match param {
            None | Some("term" | "value") => value = object_value(p)?,
            Some("fuzziness") => fuzziness = parse_fuzziness(p)?,
            Some("prefix_length") => prefix_length = int_value(p)?,
            Some("max_expansions") => max_expansions = int_value(p)?,
            Some("transpositions") => transpositions = bool_value(p)?,
            Some(param) => {
                if !common(p, "fuzzy", param, &mut boost)? {
                    return Err(does_not_support(p, "fuzzy", param));
                }
            }
        }
        Ok(())
    })?;
    let Some(field) = field.filter(|f| !f.is_empty()) else {
        return Err(DslError::illegal("field name cannot be null or empty"));
    };
    let Some(value) = value else {
        return Err(DslError::illegal("query value cannot be null"));
    };
    Ok(More::Fuzzy {
        field: owned(field),
        value,
        fuzziness,
        prefix_length,
        max_expansions,
        transpositions,
        boost,
    })
}

/// `ExistsQueryBuilder.fromXContent`.
pub fn parse_exists(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut field = None;
    let mut boost = 1.0;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(k) if k.is_value() => match current {
                "field" => field = Some(p.text()),
                "boost" => boost = float_value(p)?,
                "_name" => return Err(DslError::unsupported("[exists] [_name]")),
                _ => return Err(does_not_support(p, "exists", current)),
            },
            Some(k) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!("[exists] unknown token [{}] after [{current}]", k.name()),
                ));
            }
        }
    }
    let Some(field) = field else {
        return Err(DslError::parsing(
            p.location(),
            "[exists] must be provided with a [field]",
        ));
    };
    if field.is_empty() {
        return Err(DslError::illegal("field name is null or empty"));
    }
    Ok(More::Exists {
        field: owned(field),
        boost,
    })
}

/// `IdsQueryBuilder.fromXContent`: its `PARSER`, an `ObjectParser` of
/// `values`, `boost` and `_name`, whose `IllegalArgumentException`s (an
/// `XContentParseException` is one) become a `ParsingException` at the
/// token the parser stopped on.
pub fn parse_ids(p: &mut Parser<'_>) -> Result<More, DslError> {
    ids_object(p).map_err(|e| match e {
        DslError::Failure(f)
            if matches!(
                f.kind,
                "illegal_argument_exception"
                    | "x_content_parse_exception"
                    | "number_format_exception"
            ) =>
        {
            DslError::Failure(
                crate::dsl::error::parsing_failure(p.location(), f.reason.clone()).caused_by(f),
            )
        }
        other => other,
    })
}

fn ids_object(p: &mut Parser<'_>) -> Result<More, DslError> {
    const FIELDS: &[&str] = &["values", "boost", "_name"];
    // ALLOC: none until an id is read; bounded by the body's recording.
    let mut values = Vec::new();
    let mut boost = 1.0;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {}
            Some(_) => {
                return Err(DslError::xcontent(
                    p.location(),
                    "[ids] no field found",
                    None,
                ));
            }
        }
        let name = p.text();
        let at = p.location();
        let token = p.next_token();
        if !FIELDS.contains(&name) {
            return Err(DslError::xcontent(
                at,
                // ALLOC: error path; one message.
                &format!(
                    "[ids] unknown field [{name}]{}",
                    crate::dsl::values::suggest(name, FIELDS)
                ),
                None,
            ));
        }
        match (name, token) {
            ("values", Some(Kind::StartArray)) => loop {
                match p.next_token() {
                    Some(Kind::EndArray) | None => break,
                    Some(Kind::String | Kind::Number | Kind::Bool) => {
                        values.push(owned(p.text()));
                    }
                    Some(k) => {
                        return Err(DslError::xcontent(
                            p.location(),
                            // ALLOC: error path; one message.
                            &format!("[ids] values doesn't support values of type: {}", k.name()),
                            None,
                        ));
                    }
                }
            },
            ("boost", Some(Kind::Number | Kind::String)) => boost = float_value(p)?,
            ("_name", Some(Kind::String)) => return Err(DslError::unsupported("[ids] [_name]")),
            (_, token) => {
                return Err(DslError::xcontent(
                    p.location(),
                    // ALLOC: error path; one message.
                    &format!(
                        "[ids] {name} doesn't support values of type: {}",
                        token.map_or("null", Kind::name)
                    ),
                    None,
                ));
            }
        }
    }
    Ok(More::Ids { values, boost })
}
