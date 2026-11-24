//! `composite`'s body: `CompositeAggregationBuilder`'s `ConstructingObjectParser`,
//! `CompositeValuesSourceParserHelper.fromXContent` and the sources' own
//! `ObjectParser`s, at OpenSearch 3.10.0.

use crate::aggs::bucket::composite::{
    AfterValue, CompositeSpec, MissingOrder, SourceKind, SourceSpec,
};
use crate::aggs::bucket::date_histogram::{DateRounding, RegionZone, RoundingKind};
use crate::aggs::bucket::date_histogram_parse::{
    calendar_unit, offset_millis, time_value_millis, zone_millis,
};
use crate::aggs::bucket::histogram_parse::double_value;
use crate::aggs::metric::parse::long_value;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, int_value, suggest};

/// Every key `composite` declares.
const KEYS: &[&str] = &["sources", "size", "after"];

/// Every key a `terms` source declares.
const TERMS_KEYS: &[&str] = &[
    "field",
    "missing_bucket",
    "missing_order",
    "value_type",
    "script",
    "order",
];

/// Every key a `histogram` source declares.
const HISTOGRAM_KEYS: &[&str] = &[
    "interval",
    "field",
    "missing_bucket",
    "missing_order",
    "value_type",
    "script",
    "order",
];

/// Every key a `date_histogram` source declares.
const DATE_KEYS: &[&str] = &[
    "format",
    "calendar_interval",
    "fixed_interval",
    "interval",
    "offset",
    "time_zone",
    "field",
    "missing_bucket",
    "missing_order",
    "value_type",
    "script",
    "order",
];

/// `ensureExpectedToken`: the next token is `want`.
fn expect(p: &mut Parser<'_>, want: Kind) -> Result<(), DslError> {
    let got = p.next_token();
    if got == Some(want) {
        return Ok(());
    }
    Err(DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!(
            "Failed to parse object: expecting token of type [{}] but found [{}]",
            want.name(),
            got.map_or("null", Kind::name)
        ),
    ))
}

/// A `composite` body, the parser on its start.
pub fn parse_composite_body(p: &mut Parser<'_>, name: &str) -> Result<CompositeSpec, DslError> {
    // ALLOC: one entry a source, bounded by the body's recording.
    let mut sources: Option<Vec<SourceSpec>> = None;
    let mut size = 10usize;
    let mut after = None;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {}
            Some(other) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "Failed to parse object: expecting token of type [FIELD_NAME] but found \
                         [{}]",
                        other.name()
                    ),
                ));
            }
        }
        let key = p.text();
        let key_at = p.location();
        if !KEYS.contains(&key) {
            return Err(DslError::xcontent(
                key_at,
                // ALLOC: error path; one message.
                &format!("[composite] unknown field [{key}]{}", suggest(key, KEYS)),
                None,
            ));
        }
        let kind = p.next_token();
        match (key, kind) {
            ("sources", Some(Kind::StartArray)) => sources = Some(parse_sources(p)?),
            ("size", Some(Kind::Number | Kind::String)) => {
                let n = int_value(p).map_err(|e| failed(p, key, e))?;
                size = usize::try_from(n).ok().filter(|n| *n > 0).ok_or_else(|| {
                    // ALLOC: error path; one message.
                    DslError::unsupported(&format!(
                        "a [size] of [{n}] in the [composite] aggregation [{name}]"
                    ))
                })?;
            }
            ("after", Some(Kind::StartObject)) => after = Some(parse_after(p)?),
            (_, other) => {
                return Err(DslError::xcontent(
                    p.location(),
                    // ALLOC: error path; one message.
                    &format!(
                        "[composite] {key} doesn't support values of type: {}",
                        other.map_or("null", Kind::name)
                    ),
                    None,
                ));
            }
        }
    }
    let sources = sources.ok_or_else(|| DslError::illegal("Required [sources]"))?;
    validate(&sources)?;
    Ok(CompositeSpec {
        sources,
        size,
        after,
    })
}

fn failed(p: &Parser<'_>, key: &str, cause: DslError) -> DslError {
    DslError::xcontent(
        p.location(),
        // ALLOC: error path; one message.
        &format!("[composite] failed to parse field [{key}]"),
        Some(cause),
    )
}

/// `CompositeAggregationBuilder.validateSources`.
fn validate(sources: &[SourceSpec]) -> Result<(), DslError> {
    if sources.is_empty() {
        return Err(DslError::illegal(
            "Composite [sources] cannot be null or empty",
        ));
    }
    // ALLOC: error path; the duplicated names.
    let mut duplicates: Vec<&str> = Vec::new();
    for (i, s) in sources.iter().enumerate() {
        let seen = sources.iter().take(i).any(|o| o.name == s.name);
        if seen && !duplicates.contains(&s.name.as_str()) {
            duplicates.push(&s.name);
        }
    }
    if duplicates.is_empty() {
        return Ok(());
    }
    Err(DslError::illegal(
        // ALLOC: error path; one message.
        format!(
            "Composite source names must be unique, found duplicates: [{}]",
            duplicates.join(", ")
        ),
    ))
}

/// The array of sources, the parser past its `[`.
fn parse_sources(p: &mut Parser<'_>) -> Result<Vec<SourceSpec>, DslError> {
    // ALLOC: one entry a source, bounded by the body's recording.
    let mut out = Vec::new();
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => return Ok(out),
            Some(Kind::StartObject) => out.push(parse_source(p)?),
            Some(other) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "Failed to parse object: expecting token of type [START_OBJECT] but \
                         found [{}]",
                        other.name()
                    ),
                ));
            }
        }
    }
}

/// One source `{"name": {"type": {..}}}`, the parser past its `{`.
fn parse_source(p: &mut Parser<'_>) -> Result<SourceSpec, DslError> {
    expect(p, Kind::FieldName)?;
    // ALLOC: one name a source, bounded by the body's recording.
    let name = p.text().to_owned();
    expect(p, Kind::StartObject)?;
    expect(p, Kind::FieldName)?;
    let ty = p.text();
    let type_at = p.location();
    expect(p, Kind::StartObject)?;
    let source = match ty {
        "terms" => parse_source_body(p, name, ty, Shape::Terms)?,
        "histogram" => parse_source_body(p, name, ty, Shape::Histogram)?,
        "date_histogram" => parse_source_body(p, name, ty, Shape::Date)?,
        other => {
            return Err(DslError::parsing(
                type_at,
                // ALLOC: error path; one message.
                format!("invalid source type: {other}"),
            ));
        }
    };
    expect(p, Kind::EndObject)?;
    expect(p, Kind::EndObject)?;
    Ok(source)
}

/// Which source type a body is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Terms,
    Histogram,
    Date,
}

/// A source's body, the parser past its `{`.
fn parse_source_body(
    p: &mut Parser<'_>,
    name: String,
    ty: &str,
    shape: Shape,
) -> Result<SourceSpec, DslError> {
    let keys = match shape {
        Shape::Terms => TERMS_KEYS,
        Shape::Histogram => HISTOGRAM_KEYS,
        Shape::Date => DATE_KEYS,
    };
    let mut interval_key: Option<&'static str> = None;
    let mut date_kind: Option<RoundingKind> = None;
    let mut zone_ms = 0i64;
    let mut region: Option<RegionZone> = None;
    let mut offset = 0i64;
    let mut field: Option<String> = None;
    let mut interval: Option<f64> = None;
    let mut desc = false;
    let mut missing_bucket = false;
    let mut missing_order = MissingOrder::Default;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {}
            Some(other) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "Failed to parse object: expecting token of type [FIELD_NAME] but found \
                         [{}]",
                        other.name()
                    ),
                ));
            }
        }
        let key = p.text();
        if !keys.contains(&key) {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!("[{ty}] unknown field [{key}]{}", suggest(key, keys)),
                None,
            ));
        }
        let kind = p.next_token();
        if matches!(key, "value_type" | "script" | "format")
            || (key == "interval" && shape == Shape::Date)
        {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                // ALLOC: error path; one message.
                "a composite source's [{key}]"
            )));
        }
        let scalar = matches!(kind, Some(Kind::String | Kind::Number | Kind::Bool));
        if !scalar {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[{ty}] {key} doesn't support values of type: {}",
                    kind.map_or("null", Kind::name)
                ),
                None,
            ));
        }
        let failed = |p: &Parser<'_>, cause: DslError| {
            DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!("[{ty}] failed to parse field [{key}]"),
                Some(cause),
            )
        };
        match key {
            // ALLOC: one field name a source, bounded by the body's recording.
            "field" => field = Some(p.text().to_owned()),
            "missing_bucket" => missing_bucket = bool_value(p).map_err(|e| failed(p, e))?,
            "missing_order" => {
                missing_order = match p.text().to_ascii_lowercase().as_str() {
                    "first" => MissingOrder::First,
                    "last" => MissingOrder::Last,
                    "default" => MissingOrder::Default,
                    other => {
                        return Err(failed(
                            p,
                            // ALLOC: error path; one message.
                            DslError::illegal(format!("Unknown MissingOrder [{other}]")),
                        ));
                    }
                };
            }
            "order" => {
                desc = match p.text().to_ascii_lowercase().as_str() {
                    "asc" => false,
                    "desc" => true,
                    other => {
                        return Err(failed(
                            p,
                            // ALLOC: error path; one message.
                            DslError::illegal(format!("Unknown SortOrder [{other}]")),
                        ));
                    }
                };
            }
            "calendar_interval" | "fixed_interval" => {
                let this: &'static str = if key == "calendar_interval" {
                    "calendar_interval"
                } else {
                    "fixed_interval"
                };
                if let Some(prior) = interval_key
                    && prior != this
                {
                    return Err(failed(
                        p,
                        DslError::illegal(
                            // ALLOC: error path; one message.
                            format!("Cannot use [{this}] with [{prior}] configuration option."),
                        ),
                    ));
                }
                interval_key = Some(this);
                let text = p.text();
                date_kind = Some(if this == "calendar_interval" {
                    let unit = calendar_unit(text).ok_or_else(|| {
                        failed(
                            p,
                            DslError::illegal(
                                // ALLOC: error path; one message.
                                format!(
                                    "The supplied interval [{text}] could not be parsed as a \
                                     calendar interval."
                                ),
                            ),
                        )
                    })?;
                    RoundingKind::Unit(unit)
                } else {
                    let ms = time_value_millis(text, "date_histogram.fixedInterval")
                        .map_err(|m| failed(p, DslError::illegal(m)))?;
                    if ms < 1 {
                        return Err(failed(
                            p,
                            DslError::illegal("Zero or negative time interval not supported"),
                        ));
                    }
                    RoundingKind::Interval(ms)
                });
            }
            "time_zone" => {
                region = None;
                zone_ms = if kind == Some(Kind::Number) {
                    long_value(Kind::Number, p.text())
                        .map_err(|e| failed(p, e))?
                        .saturating_mul(3_600_000)
                } else if let Some(fixed) = zone_millis(p.text()) {
                    fixed
                } else if let Some(named) = RegionZone::named(p.text()) {
                    region = Some(named);
                    0
                } else {
                    // ALLOC: error path; one message.
                    return Err(DslError::unsupported(&format!(
                        "a composite source's time zone [{}]",
                        p.text()
                    )));
                };
            }
            "offset" => {
                offset = if kind == Some(Kind::Number) {
                    long_value(Kind::Number, p.text()).map_err(|e| failed(p, e))?
                } else {
                    offset_millis(p.text()).map_err(|m| failed(p, DslError::illegal(m)))?
                };
            }
            _ => {
                let v = double_value(p).map_err(|e| failed(p, e))?;
                if v.is_nan() || v <= 0.0 {
                    return Err(failed(
                        p,
                        DslError::illegal(
                            "[interval] must be greater than 0 for [histogram] source",
                        ),
                    ));
                }
                interval = Some(v);
            }
        }
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    if !missing_bucket && missing_order != MissingOrder::Default {
        return Err(DslError::illegal(
            "missing_order require missing_bucket is true",
        ));
    }
    let kind = match shape {
        Shape::Histogram => SourceKind::Histogram {
            interval: interval.ok_or_else(|| {
                DslError::illegal("[interval] must be greater than 0 for [histogram] source")
            })?,
        },
        Shape::Terms => SourceKind::Terms,
        Shape::Date => SourceKind::DateHistogram {
            rounding: DateRounding {
                kind: date_kind.ok_or_else(|| {
                    DslError::illegal("Invalid interval specified, must be non-null and non-empty")
                })?,
                zone_ms,
                region,
                offset,
            },
        },
    };
    Ok(SourceSpec {
        name,
        field,
        kind,
        desc,
        missing_bucket,
        missing_order,
    })
}

/// The `after` object, the parser past its `{`: one entry a key.
fn parse_after(p: &mut Parser<'_>) -> Result<Vec<(String, AfterValue)>, DslError> {
    // ALLOC: one entry a key, bounded by the body's recording.
    let mut out = Vec::new();
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(out),
            Some(Kind::FieldName) => {}
            Some(other) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "Failed to parse object: expecting token of type [FIELD_NAME] but found \
                         [{}]",
                        other.name()
                    ),
                ));
            }
        }
        // ALLOC: one name an `after` key, bounded by the body's recording.
        let key = p.text().to_owned();
        let value = match p.next_token() {
            Some(Kind::Null) => AfterValue::Null,
            // ALLOC: one value an `after` key, bounded by the body's recording.
            Some(Kind::String) => AfterValue::Str(p.text().to_owned()),
            // ALLOC: as above.
            Some(Kind::Number) => AfterValue::Number(p.text().to_owned()),
            _ => {
                return Err(DslError::unsupported(
                    "an [after] value that is not a string, a number or null",
                ));
            }
        };
        out.push((key, value));
    }
}
