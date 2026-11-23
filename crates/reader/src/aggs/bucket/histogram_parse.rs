//! `histogram`'s body: `HistogramAggregationBuilder`'s `ObjectParser`
//! (`ValuesSourceAggregationBuilder.declareFields` and its own keys), its
//! setters' checks and `DoubleBounds`, at OpenSearch 3.10.0.

use crate::aggs::bucket::histogram::{HistogramOrder, HistogramSpec};
use crate::aggs::metric::double::write_java_double;
use crate::aggs::metric::parse::long_value;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, suggest};

/// Every key `histogram` declares.
const KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "format",
    "script",
    "interval",
    "offset",
    "keyed",
    "min_doc_count",
    "extended_bounds",
    "hard_bounds",
    "order",
];

/// `parser.doubleValue()`: a number or a string holding one.
pub(crate) fn double_value(p: &Parser<'_>) -> Result<f64, DslError> {
    match p.current_token() {
        Some(Kind::Number | Kind::String) => p.text().trim().parse::<f64>().map_err(|_| {
            DslError::Failure(crate::failure::Failure::plain(
                "number_format_exception",
                // ALLOC: error path; one message.
                format!("For input string: \"{}\"", p.text()),
            ))
        }),
        _ => Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("Current token ({}) not numeric", p.text()),
        )),
    }
}

/// A double as `Double.toString` writes it, for a message.
fn java_text(d: f64) -> String {
    // ALLOC: error path; one short message.
    let mut out = Vec::new();
    // Writing into a vector cannot fail.
    let _ = write_java_double(&mut out, d);
    String::from_utf8(out).unwrap_or_default()
}

/// `DoubleBounds.PARSER` over an object, the parser on its start.
fn parse_bounds(p: &mut Parser<'_>) -> Result<(Option<f64>, Option<f64>), DslError> {
    let mut min = None;
    let mut max = None;
    let mut key = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => key = p.text(),
            Some(Kind::Null) => {}
            Some(_) => {
                let value = double_value(p)?;
                match key {
                    "min" => min = Some(value),
                    "max" => max = Some(value),
                    _ => {
                        return Err(DslError::illegal(
                            // ALLOC: error path; one message.
                            format!("[double_bounds] unknown field [{key}]"),
                        ));
                    }
                }
            }
        }
    }
    for (name, bound) in [("min", min), ("max", max)] {
        if bound.is_some_and(|b| !b.is_finite()) {
            return Err(DslError::illegal(
                // ALLOC: error path; one message.
                format!(
                    "{name} bound must be finite, got: {}",
                    java_text(bound.unwrap_or(0.0))
                ),
            ));
        }
    }
    if let (Some(lo), Some(hi)) = (min, max)
        && hi < lo
    {
        return Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!(
                "max bound [{}] must be greater than min bound [{}]",
                java_text(hi),
                java_text(lo)
            ),
        ));
    }
    Ok((min, max))
}

/// The field and options of a `histogram` body, the parser on its start.
pub fn parse_histogram_body(
    p: &mut Parser<'_>,
    name: &str,
) -> Result<(String, HistogramSpec), DslError> {
    let mut field: Option<String> = None;
    let mut spec = HistogramSpec::default();
    let mut key = "";
    let mut key_at = p.location();
    loop {
        let kind = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                key_at = p.location();
                continue;
            }
            Some(kind) => kind,
        };
        if !KEYS.contains(&key) {
            return Err(DslError::xcontent(
                key_at,
                // ALLOC: error path; one message.
                &format!("[histogram] unknown field [{key}]{}", suggest(key, KEYS)),
                None,
            ));
        }
        if matches!(key, "missing" | "value_type" | "format" | "script") {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [histogram] aggregation's [{key}]"
            )));
        }
        let accepted = match key {
            "field" => kind == Kind::String,
            "keyed" => matches!(kind, Kind::Bool | Kind::String),
            "extended_bounds" | "hard_bounds" => kind == Kind::StartObject,
            "order" => matches!(kind, Kind::StartObject | Kind::StartArray),
            _ => matches!(kind, Kind::Number | Kind::String),
        };
        if !accepted {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[histogram] {key} doesn't support values of type: {}",
                    kind.name()
                ),
                None,
            ));
        }
        let failed = |p: &Parser<'_>, cause: DslError| {
            DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!("[histogram] failed to parse field [{key}]"),
                Some(cause),
            )
        };
        match key {
            // ALLOC: one field name an aggregation, bounded by the body's recording.
            "field" => field = Some(p.text().to_owned()),
            "interval" => {
                let v = double_value(p).map_err(|e| failed(p, e))?;
                if v <= 0.0 || v.is_nan() {
                    return Err(failed(
                        p,
                        DslError::illegal(
                            // ALLOC: error path; one message.
                            format!("[interval] must be >0 for histogram aggregation [{name}]"),
                        ),
                    ));
                }
                spec.interval = v;
            }
            "offset" => spec.offset = double_value(p).map_err(|e| failed(p, e))?,
            "keyed" => spec.keyed = bool_value(p).map_err(|e| failed(p, e))?,
            "min_doc_count" => {
                let n = long_value(kind, p.text()).map_err(|e| failed(p, e))?;
                spec.min_doc_count = u64::try_from(n).map_err(|_| {
                    failed(
                        p,
                        DslError::illegal(
                            // ALLOC: error path; one message.
                            format!(
                                "[minDocCount] must be greater than or equal to 0. Found [{n}] \
                                 in [{name}]"
                            ),
                        ),
                    )
                })?;
            }
            "extended_bounds" => {
                spec.extended_bounds = Some(parse_bounds(p).map_err(|e| failed(p, e))?);
            }
            "hard_bounds" => spec.hard_bounds = parse_bounds(p).map_err(|e| failed(p, e))?,
            _ => spec.order = parse_order(p, kind, "histogram")?,
        }
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    check_bounds(&spec)?;
    Ok((field, spec))
}

/// `HistogramAggregationBuilder.innerBuild`'s check that the extended bounds
/// lie inside the hard ones.
fn check_bounds(spec: &HistogramSpec) -> Result<(), DslError> {
    let Some((ext_min, ext_max)) = spec.extended_bounds else {
        return Ok(());
    };
    let (hard_min, hard_max) = spec.hard_bounds;
    let outside = matches!((hard_max, ext_max), (Some(h), Some(e)) if h < e)
        || matches!((hard_min, ext_min), (Some(h), Some(e)) if h > e);
    if !outside {
        return Ok(());
    }
    // ALLOC: error path; short messages.
    let text = |d: Option<f64>| d.map_or_else(|| "null".to_owned(), java_text);
    Err(DslError::illegal(
        // ALLOC: error path; one message.
        format!(
            "Extended bounds have to be inside hard bounds, hard bounds: [{}--{}], extended \
             bounds: [{}--{}]",
            text(hard_min),
            text(hard_max),
            text(ext_min),
            text(ext_max)
        ),
    ))
}

/// `InternalOrder.Parser.parseOrderParam` over one object, or an array of
/// one, the parser on its start (`kind`): `_count` or `_key`, `asc` or
/// `desc`. A sub-aggregation's path and several criteria are not served.
pub(super) fn parse_order(
    p: &mut Parser<'_>,
    kind: Kind,
    agg: &str,
) -> Result<HistogramOrder, DslError> {
    // ALLOC: error path; one message.
    let several = format!("an [order] of several criteria in a [{agg}] aggregation");
    let array = kind == Kind::StartArray;
    let mut order: Option<HistogramOrder> = None;
    if array {
        loop {
            match p.next_token() {
                Some(Kind::EndArray) | None => break,
                Some(Kind::StartObject) if order.is_none() => order = Some(parse_one(p, agg)?),
                Some(_) => return Err(DslError::unsupported(&several)),
            }
        }
        return order.ok_or_else(|| DslError::unsupported(&several));
    }
    parse_one(p, agg)
}

/// One criterion's object, the parser on its start.
fn parse_one(p: &mut Parser<'_>, agg: &str) -> Result<HistogramOrder, DslError> {
    let mut by: Option<&'static str> = None;
    let mut ascending = false;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                if by.is_some() {
                    // ALLOC: error path; one message.
                    return Err(DslError::unsupported(&format!(
                        "an [order] of several criteria in a [{agg}] aggregation"
                    )));
                }
                by = match p.text() {
                    "_count" => Some("_count"),
                    "_key" | "_term" => Some("_key"),
                    _ => {
                        // ALLOC: error path; one message.
                        return Err(DslError::unsupported(&format!(
                            "an [order] by a sub-aggregation in a [{agg}] aggregation"
                        )));
                    }
                };
            }
            Some(Kind::String) => {
                ascending = match p.text() {
                    "asc" => true,
                    "desc" => false,
                    other => {
                        return Err(DslError::illegal(
                            // ALLOC: error path; one message.
                            format!("Unknown order direction [{other}]"),
                        ));
                    }
                };
            }
            Some(other) => {
                return Err(DslError::illegal(
                    // ALLOC: error path; one message.
                    format!(
                        "Unexpected token [{}] in an order of a [{agg}] aggregation",
                        other.name()
                    ),
                ));
            }
        }
    }
    Ok(match (by, ascending) {
        (Some("_key"), true) => HistogramOrder::KeyAsc,
        (Some("_key"), false) => HistogramOrder::KeyDesc,
        (_, true) => HistogramOrder::CountAsc,
        (_, false) => HistogramOrder::CountDesc,
    })
}
