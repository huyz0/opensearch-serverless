//! `function_score`, as `FunctionScoreQueryBuilder.fromXContent`,
//! `FieldValueFactorFunctionBuilder.fromXContent` and
//! `DecayFunctionParser.parse` read it at OpenSearch 3.10.0.
//!
//! Served: `weight`, `field_value_factor`, `gauss`, `exp`, `linear`,
//! `script_score` and `random_score` as a single function or in `functions`,
//! each under an optional `filter`, with `score_mode`, `boost_mode`,
//! `max_boost`, `min_score` and `boost`. Refused, as not supported by this
//! node yet: `_name`, and a decay on a geo point field or on several fields.

use lucene_engine::function_score::{Combine, Decay, Modifier, MultiValue, ScoreMode};

use crate::dsl::error::{DslError, parsing_failure};
use crate::dsl::queries::More;
use crate::dsl::queries::function_score_modes::{boost_mode, modifier, multi_value, score_mode};
use crate::dsl::query::{Query, parse_inner};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{Value, float_value, object_value, text_or_null};
use crate::failure::Failure;

/// A parsed `function_score` query.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionScore {
    /// The query the functions score; match-all when absent.
    pub query: Option<Query>,
    /// The functions, none for a plain `query`.
    pub functions: Vec<Entry>,
    /// How the functions' scores combine.
    pub score_mode: ScoreMode,
    /// How the combined score joins the query's.
    pub boost_mode: Combine,
    /// The most the combined functions' score is.
    pub max_boost: f32,
    /// The least a document's score is for it to match.
    pub min_score: Option<f32>,
}

/// One function, its filter and weight (`FilterFunctionBuilder`).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The documents it applies to; all when absent.
    pub filter: Option<Query>,
    /// The weight the function's score is multiplied by.
    pub weight: Option<f32>,
    /// The function, none for a bare `weight`.
    pub function: Option<Function>,
}

/// A scoring function this node serves.
#[derive(Clone, Debug, PartialEq)]
pub enum Function {
    /// `field_value_factor`.
    FieldValue {
        /// The field.
        field: String,
        /// The factor the value is multiplied by.
        factor: f32,
        /// The modifier applied after it.
        modifier: Modifier,
        /// The value of a document without one.
        missing: Option<f64>,
    },
    /// `gauss`, `exp` or `linear`.
    Decay {
        /// Which curve.
        decay: Decay,
        /// The field.
        field: String,
        /// The field's parameters as written: `origin`, `scale`, `offset`,
        /// `decay`. Validated against the field's type where the query is
        /// built, as OpenSearch does at the shard.
        params: Vec<(String, Value)>,
        /// Which of a document's values is used.
        mode: MultiValue,
    },
    /// `script_score` over an expression.
    Script(super::function_score_script::ScriptScore),
    /// `random_score`.
    Random {
        /// The seed as `RandomScoreFunctionBuilder` holds it (an int is
        /// kept, a long and a string hashed); none hashes the document id.
        seed: Option<i32>,
        /// The field the seed hashes with; `_id` when a seed has none.
        field: Option<String>,
    },
}

/// The unknown function `name`: a `ParsingException` caused by the
/// registry's `NamedObjectNotFoundException`.
fn unknown_function(p: &Parser<'_>, name: &str) -> DslError {
    let at = p.location();
    DslError::Failure(
        parsing_failure(
            at,
            "unknown query [function_score] did you mean any of [random_score, script_score]?",
        )
        .caused_by(Failure::plain(
            "named_object_not_found_exception",
            // ALLOC: error path; one message.
            format!("[{}:{}] unknown field [{name}]", at.line, at.column),
        )),
    )
}

fn not_supported(p: &Parser<'_>, name: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("failed to parse [function_score] query. field [{name}] is not supported"),
    )
}

fn number_format(text: &str) -> DslError {
    DslError::Failure(Failure::plain(
        "number_format_exception",
        // ALLOC: error path; one message.
        format!("For input string: \"{text}\""),
    ))
}

/// `parser.doubleValue()`.
fn double_value(p: &Parser<'_>) -> Result<f64, DslError> {
    match p.current_token() {
        Some(Kind::Number | Kind::String) => p
            .text()
            .trim()
            .parse::<f64>()
            .map_err(|_| number_format(p.text())),
        _ => Err(number_format(p.text())),
    }
}

/// `FieldValueFactorFunctionBuilder.fromXContent`: on the object's start.
fn parse_field_value_factor(p: &mut Parser<'_>) -> Result<Function, DslError> {
    let mut field = None;
    let mut factor = 1.0;
    let mut mode = Modifier::None;
    let mut missing = None;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(k) if k.is_value() => match current {
                // ALLOC: one name a field value function.
                "field" => field = Some(p.text().to_owned()),
                "factor" => factor = float_value(p)?,
                "modifier" => mode = modifier(p.text())?,
                "missing" => missing = Some(double_value(p)?),
                _ => return Err(does_not_support(p, current)),
            },
            Some(_) => return Err(does_not_support(p, current)),
        }
    }
    let Some(field) = field else {
        return Err(DslError::parsing(
            p.location(),
            "[field_value_factor] required field 'field' missing",
        ));
    };
    Ok(Function::FieldValue {
        field,
        factor,
        modifier: mode,
        missing,
    })
}

fn does_not_support(p: &Parser<'_>, name: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("field_value_factor query does not support [{name}]"),
    )
}

/// `DecayFunctionParser.parse`: on the object's start.
fn parse_decay(decay: Decay, p: &mut Parser<'_>) -> Result<Function, DslError> {
    let mut field = None;
    // ALLOC: one entry a decay parameter, four at most are valid.
    let mut params = Vec::new();
    let mut mode = MultiValue::Min;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartObject) if field.is_none() => {
                // ALLOC: one name a decay function.
                field = Some(current.to_owned());
                parse_params(p, &mut params)?;
            }
            Some(k) if k.is_value() && current == "multi_value_mode" => {
                mode = multi_value(p.text())?;
            }
            Some(_) => {
                return Err(DslError::unsupported(
                    // ALLOC: error path; one message.
                    &format!("[function_score] a decay's [{current}]"),
                ));
            }
        }
    }
    let Some(field) = field else {
        return Err(DslError::unsupported(
            "[function_score] a decay of no field",
        ));
    };
    Ok(Function::Decay {
        decay,
        field,
        params,
        mode,
    })
}

/// The decay field's parameters, as written.
fn parse_params(p: &mut Parser<'_>, out: &mut Vec<(String, Value)>) -> Result<(), DslError> {
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(()),
            Some(Kind::FieldName) => current = p.text(),
            Some(k) if k.is_value() => {
                let Some(v) = object_value(p)? else {
                    return Err(DslError::unsupported("[function_score] a null decay value"));
                };
                // ALLOC: one entry a parameter, four at most are valid.
                out.push((current.to_owned(), v));
            }
            Some(_) => {
                return Err(DslError::unsupported(
                    // ALLOC: error path; one message.
                    &format!("[function_score] the decay parameter [{current}] as an object"),
                ));
            }
        }
    }
}

/// A function by its name, the parser on its object's start.
fn parse_function(name: &str, p: &mut Parser<'_>) -> Result<Function, DslError> {
    match name {
        "field_value_factor" => parse_field_value_factor(p),
        "gauss" => parse_decay(Decay::Gauss, p),
        "exp" => parse_decay(Decay::Exp, p),
        "linear" => parse_decay(Decay::Linear, p),
        "script_score" => super::function_score_script::parse(p).map(Function::Script),
        "random_score" => super::function_score_random::parse(p),
        _ => Err(unknown_function(p, name)),
    }
}

impl Entry {
    /// `FilterFunctionBuilder`'s constructor check.
    fn checked(self) -> Result<Entry, DslError> {
        if self.weight.is_some_and(|w| w < 0.0) {
            return Err(DslError::illegal(
                "[weight] cannot be negative for a filtering function",
            ));
        }
        Ok(self)
    }
}

/// One element of `functions`: the parser is on its start.
fn parse_entry(p: &mut Parser<'_>) -> Result<Entry, DslError> {
    let mut entry = Entry {
        filter: None,
        weight: None,
        function: None,
    };
    let mut found: Option<String> = None;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartObject) if current == "filter" => entry.filter = Some(parse_inner(p)?),
            Some(Kind::StartObject) => {
                if let Some(first) = &found {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!(
                            "failed to parse function_score functions. already found [{first}], now encountering [{current}]."
                        ),
                    ));
                }
                entry.function = Some(parse_function(current, p)?);
                // ALLOC: one name an entry.
                found = Some(current.to_owned());
            }
            Some(k) if k.is_value() && current == "weight" => entry.weight = Some(float_value(p)?),
            Some(_) => return Err(not_supported(p, current)),
        }
    }
    if entry.function.is_none() && entry.weight.is_none() {
        return Err(DslError::parsing(
            p.location(),
            "failed to parse [function_score] query. an entry in functions list is missing a function.",
        ));
    }
    entry.checked()
}

fn array_element_error(p: &Parser<'_>, got: Kind) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!(
            "failed to parse [START_OBJECT]. malformed query, expected a [{}] while parsing functions but got a [function_score] instead",
            got.name()
        ),
    )
}

/// `FunctionScoreQueryBuilder.fromXContent`.
pub fn parse_function_score(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut out = FunctionScore {
        query: None,
        // ALLOC: one entry a function, at most the body's entries.
        functions: Vec::new(),
        score_mode: ScoreMode::Multiply,
        boost_mode: Combine::Multiply,
        max_boost: f32::MAX,
        min_score: None,
    };
    let mut boost = 1.0;
    let mut single: Option<String> = None;
    let mut array = false;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartObject) if current == "query" => out.query = Some(parse_inner(p)?),
            Some(Kind::StartObject) => {
                reject_second(p, &single, array, current, true)?;
                let function = parse_function(current, p)?;
                // ALLOC: one name a single function.
                single = Some(current.to_owned());
                out.functions.push(Entry {
                    filter: None,
                    weight: None,
                    function: Some(function),
                });
            }
            Some(Kind::StartArray) if current == "functions" => {
                reject_second(p, &single, array, current, false)?;
                array = true;
                loop {
                    match p.next_token() {
                        Some(Kind::EndArray) | None => break,
                        Some(Kind::StartObject) => out.functions.push(parse_entry(p)?),
                        Some(other) => return Err(array_element_error(p, other)),
                    }
                }
            }
            Some(Kind::StartArray) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "failed to parse [function_score] query. array [{current}] is not supported"
                    ),
                ));
            }
            Some(_) => match current {
                "boost" => boost = float_value(p)?,
                "score_mode" => out.score_mode = score_mode(text_or_null(p).unwrap_or("null"))?,
                "boost_mode" => out.boost_mode = boost_mode(text_or_null(p).unwrap_or("null"))?,
                "max_boost" => out.max_boost = float_value(p)?,
                "min_score" => out.min_score = Some(float_value(p)?),
                "_name" => crate::dsl::values::name_value(p)?,
                "weight" => {
                    reject_second(p, &single, array, current, true)?;
                    // ALLOC: one name a single function.
                    single = Some(current.to_owned());
                    out.functions.push(
                        Entry {
                            filter: None,
                            weight: Some(float_value(p)?),
                            function: None,
                        }
                        .checked()?,
                    );
                }
                _ => {
                    if let Some(first) = &single {
                        return Err(already_found(p, first, current));
                    }
                    return Err(not_supported(p, current));
                }
            },
        }
    }
    if boost < 0.0 {
        return Err(DslError::unsupported("[function_score] a negative [boost]"));
    }
    // ALLOC: one box a function-score query.
    Ok(More::FunctionScore(Box::new(out), boost))
}

fn already_found(p: &Parser<'_>, first: &str, now: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!(
            "failed to parse [function_score] query. already found function [{first}], now encountering [{now}]. use [functions] array if you want to define several functions."
        ),
    )
}

/// A single function or the array after another: `object` says which of
/// OpenSearch's two messages a single function earlier takes.
fn reject_second(
    p: &Parser<'_>,
    single: &Option<String>,
    array: bool,
    now: &str,
    object: bool,
) -> Result<(), DslError> {
    if array {
        return Err(DslError::parsing(
            p.location(),
            // ALLOC: error path; one message.
            format!(
                "failed to parse [function_score] query. [you can either define [functions] array or a single function, not both. already found [functions] array, now encountering [{now}].]"
            ),
        ));
    }
    let Some(first) = single else {
        return Ok(());
    };
    if object {
        return Err(already_found(p, first, now));
    }
    Err(DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!(
            "failed to parse [function_score] query. [you can either define [functions] array or a single function, not both. already found [{first}], now encountering [{now}].]"
        ),
    ))
}
