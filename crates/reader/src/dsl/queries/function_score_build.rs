//! `function_score` as a Lucene query: `FunctionScoreQueryBuilder.doToQuery`
//! with each function's `toFunction` (`FieldValueFactorFunctionBuilder`,
//! `DecayFunctionBuilder`, `DecayFunctionParser.parseVariable`) at
//! OpenSearch 3.10.0, over the engine's OpenSearch-arithmetic function-score
//! node.

use std::sync::Arc;

use lucene_engine::function_score::{
    Combine, Decay, Filtered, Function as Engine, Numeric, OpenSearchFunctionScore,
};
use lucene_search::Clause;
use lucene_search::extended_query::ExtendedQuery;
use lucene_search::function::FunctionScoreQuery;
use lucene_search::values_source::from_clause;

use crate::date;
use crate::dsl::error::parsing_failure;
use crate::dsl::queries::function_score::{Entry, Function, FunctionScore};
use crate::dsl::values::Value;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, build, match_all, unsupported};
use crate::mapping::FieldType;
use oss_rest::json::Location;

/// `FunctionScoreQueryBuilder.doToQuery`.
pub fn function_score(cx: &QueryContext<'_>, q: &FunctionScore) -> Result<Clause, Failure> {
    let sub = match &q.query {
        Some(query) => build(query, cx)?,
        None => match_all(cx),
    };
    if q.functions.is_empty() {
        return Ok(sub);
    }
    // ALLOC: one entry a function, at most the body's recording.
    let mut functions = Vec::with_capacity(q.functions.len());
    for entry in &q.functions {
        functions.push(filtered(cx, entry)?);
    }
    let functions_scripted = functions.iter().any(|f| f.function.scripted());
    let needs_sub =
        q.boost_mode != Combine::Replace || functions.iter().any(|f| f.function.needs_score());
    // ALLOC: one key a function-score query.
    let key = format!(
        "{:?}/{:?}/{}/{}",
        q.boost_mode,
        q.score_mode,
        q.max_boost,
        // ALLOC: one description a function, at most the body's recording.
        functions
            .iter()
            .map(|f| format!("{:?}{:?}", f.filter, f.function))
            .collect::<Vec<_>>()
            .join(";")
    );
    let source = OpenSearchFunctionScore {
        combine: q.boost_mode,
        score_mode: q.score_mode,
        max_boost: q.max_boost,
        functions,
        scripted: functions_scripted,
        needs_sub,
        key,
    };
    // ALLOC: one box a function-score query.
    Ok(Clause::Extended(Box::new(ExtendedQuery::FunctionScore(
        FunctionScoreQuery::new(sub, Arc::new(source)),
    ))))
}

fn filtered(cx: &QueryContext<'_>, entry: &Entry) -> Result<Filtered, Failure> {
    let filter = entry.filter.as_ref().map(|f| build(f, cx)).transpose()?;
    let inner = match &entry.function {
        Some(f) => function(cx, f)?,
        None => Engine::One,
    };
    let function = match entry.weight {
        Some(weight) => Engine::Weight {
            weight,
            // ALLOC: one box a weighted function.
            inner: Box::new(inner),
        },
        None => inner,
    };
    Ok(Filtered {
        source: filter.clone().map(from_clause),
        filter,
        function,
    })
}

fn function(cx: &QueryContext<'_>, f: &Function) -> Result<Engine, Failure> {
    match f {
        Function::Script(script) => super::function_score_script::build(cx, script),
        Function::FieldValue {
            field,
            factor,
            modifier,
            missing,
        } => {
            match cx.mapping.get(field).map(|f| &f.ty) {
                None if missing.is_none() => {
                    return Err(wrapped(
                        "exception",
                        // ALLOC: error path; one message.
                        format!(
                            "Unable to find a field mapper for field [{field}]. No 'missing' value defined."
                        ),
                    ));
                }
                None => {}
                Some(ty) => numeric(ty, field, true)?,
            }
            Ok(Engine::FieldValue {
                // ALLOC: one name a function.
                field: field.clone(),
                // ALLOC: one name a function.
                name: field.clone(),
                numeric: Numeric::Long,
                factor: *factor,
                modifier: *modifier,
                missing: *missing,
            })
        }
        Function::Decay {
            decay,
            field,
            params,
            mode,
        } => {
            let Some(ty) = cx.mapping.get(field).map(|f| &f.ty) else {
                return Err(parsing_failure(
                    Location { line: 1, column: 0 },
                    // ALLOC: error path; one message.
                    format!("unknown field [{field}]"),
                ));
            };
            numeric(ty, field, false)?;
            let p = Params::read(params, matches!(ty, FieldType::Date))?;
            let (origin, scale, offset) = p.resolve(ty)?;
            Ok(Engine::Decay {
                decay: *decay,
                // ALLOC: one name a function.
                field: field.clone(),
                numeric: Numeric::Long,
                origin,
                scale: process_scale(*decay, scale, p.decay),
                offset,
                mode: *mode,
            })
        }
    }
}

/// An `OpenSearchException` that `QueryShardContext.toQuery` wraps in a
/// `QueryShardException`, which the shard failure then unwraps for its root.
fn wrapped(kind: &'static str, reason: impl Into<String>) -> Failure {
    let reason = reason.into();
    // ALLOC: error path; one message.
    Failure::opensearch(
        "query_shard_exception",
        format!("failed to create query: {reason}"),
    )
    .caused_by(Failure::opensearch(kind, reason))
}

/// A field whose doc values read as numbers: an integer, a long or a date.
fn numeric(ty: &FieldType, field: &str, field_value: bool) -> Result<(), Failure> {
    match ty {
        FieldType::Int { .. } | FieldType::Long | FieldType::Date => Ok(()),
        FieldType::Text | FieldType::Keyword if field_value => Err(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "Text fields are not optimised for operations that require per-document field data like aggregations and sorting, so these operations are disabled by default. Please use a keyword field instead. Alternatively, set fielddata=true on [{field}] in order to load field data by uninverting the inverted index. Note that this can use significant memory."
            ),
        )),
        // ALLOC: error path; one message.
        _ => Err(unsupported(&format!(
            // ALLOC: error path; one message.
            "scoring by the field [{field}], which is not a number or a date,"
        ))),
    }
}

/// `DecayFunction.processScale`, the scale the curve is evaluated with.
fn process_scale(decay: Decay, scale: f64, rate: f64) -> f64 {
    match decay {
        Decay::Gauss => (scale * scale) / (2.0 * rate.ln()),
        Decay::Exp => rate.ln() / scale,
        Decay::Linear => scale / (1.0 - rate),
    }
}

/// A decay field's parameters, as written.
struct Params<'a> {
    origin: Option<&'a Value>,
    scale: Option<&'a Value>,
    offset: Option<&'a Value>,
    decay: f64,
}

impl<'a> Params<'a> {
    /// `parseVariable`'s loop: the four parameters, nothing else.
    fn read(params: &'a [(String, Value)], _date: bool) -> Result<Self, Failure> {
        let mut p = Params {
            origin: None,
            scale: None,
            offset: None,
            decay: 0.5,
        };
        for (name, v) in params {
            match name.as_str() {
                "origin" => p.origin = Some(v),
                "scale" => p.scale = Some(v),
                "offset" => p.offset = Some(v),
                "decay" => p.decay = double(v.text())?,
                other => {
                    return Err(wrapped(
                        "parse_exception",
                        // ALLOC: error path; one message.
                        format!("parameter [{other}] not supported!"),
                    ));
                }
            }
        }
        Ok(p)
    }

    /// The origin, scale and offset as numbers (a date's in milliseconds),
    /// checked as `DecayFunctionBuilder` checks them.
    fn resolve(&self, ty: &FieldType) -> Result<(f64, f64, f64), Failure> {
        let (origin, scale, offset) = if matches!(ty, FieldType::Date) {
            let Some(origin) = self.origin else {
                return Err(unsupported("a date decay without an origin (now)"));
            };
            let at = date::parse_to_millis(origin.text(), false)
                .map_err(|e| e.into_failure(origin.text()))?;
            let scale = match self.scale {
                Some(s) => time_value("scale", s.text())?,
                None => {
                    return Err(wrapped(
                        "parse_exception",
                        "both [scale] and [origin] must be set for numeric fields.",
                    ));
                }
            };
            let offset = match self.offset {
                Some(s) => time_value("offset", s.text())?,
                None => 0.0,
            };
            // ARITH: none; a millisecond epoch is far inside f64's integers.
            (at as f64, scale, offset)
        } else {
            let (Some(origin), Some(scale)) = (self.origin, self.scale) else {
                return Err(wrapped(
                    "parse_exception",
                    "both [scale] and [origin] must be set for numeric fields.",
                ));
            };
            let offset = match self.offset {
                Some(o) => double(o.text())?,
                None => 0.0,
            };
            (double(origin.text())?, double(scale.text())?, offset)
        };
        let bad = |what: String| Failure::plain("illegal_argument_exception", what);
        if self.decay <= 0.0 || self.decay >= 1.0 {
            return Err(bad(
                "function_score : decay must be in the range [0..1].".into()
            ));
        }
        if scale <= 0.0 {
            return Err(bad("function_score : scale must be > 0.0.".into()));
        }
        if offset < 0.0 {
            return Err(bad("function_score : offset must be > 0.0".into()));
        }
        Ok((origin, scale, offset))
    }
}

/// `Double.parseDouble`.
fn double(text: &str) -> Result<f64, Failure> {
    text.trim().parse::<f64>().map_err(|_| {
        Failure::plain(
            "number_format_exception",
            // ALLOC: error path; one message.
            format!("For input string: \"{text}\""),
        )
    })
}

/// `TimeValue.parseTimeValue(text, setting).getMillis()`, as a double.
fn time_value(setting: &str, text: &str) -> Result<f64, Failure> {
    let unit_missing = || {
        Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "failed to parse setting [DecayFunctionParser.{setting}] with value [{text}] as a time value: unit is missing or unrecognized"
            ),
        )
    };
    let lower = text.to_lowercase();
    let lower = lower.trim();
    if lower == "0" || lower == "-1" {
        return Ok(0.0);
    }
    // The suffixes in `TimeValue.parseTimeValue`'s order.
    const UNITS: [(&str, i64, i64); 7] = [
        ("nanos", 1, 1_000_000),
        ("micros", 1, 1_000),
        ("ms", 1, 1),
        ("s", 1_000, 1),
        ("m", 60_000, 1),
        ("h", 3_600_000, 1),
        ("d", 86_400_000, 1),
    ];
    for (suffix, mul, div) in UNITS {
        let Some(digits) = lower.strip_suffix(suffix) else {
            continue;
        };
        let Ok(n) = digits.trim().parse::<i64>() else {
            // ALLOC: error path; one message.
            return Err(unsupported(&format!(
                // ALLOC: error path; one message.
                "the time value [{text}]"
            )));
        };
        let millis = n
            .checked_mul(mul)
            .and_then(|v| v.checked_div(div))
            .ok_or_else(unit_missing)?;
        // ARITH: none; an i64 of milliseconds converts with the usual rounding.
        return Ok(millis as f64);
    }
    Err(unit_missing())
}
