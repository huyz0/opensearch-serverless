//! `range`'s body: `RangeAggregationBuilder`'s `ObjectParser`
//! (`ValuesSourceAggregationBuilder.declareFields` and its own `keyed` and
//! `ranges`) and `RangeAggregator.Range.PARSER`, at OpenSearch 3.10.0, and
//! `AbstractRangeBuilder.sortRanges`.

use crate::aggs::bucket::range::{RangeDef, RangeSpec};
use crate::aggs::metric::percentiles::double_value;
use crate::date_pattern::DatePattern;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, suggest};

/// Every key `range` declares.
const KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "format",
    "script",
    "keyed",
    "ranges",
];

/// Every key `date_range` declares.
const DATE_KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "format",
    "script",
    "time_zone",
    "keyed",
    "ranges",
];

/// Every key a range declares.
const RANGE_KEYS: &[&str] = &["key", "from", "to"];

/// The field and options of a `range` body, the parser on its start.
pub fn parse_range_body(p: &mut Parser<'_>, name: &str) -> Result<(String, RangeSpec), DslError> {
    parse_body(p, name, "range")
}

/// The field and options of a `date_range` body, the parser on its start:
/// the `range`'s, each bound a number of epoch milliseconds or a date.
pub fn parse_date_range_body(
    p: &mut Parser<'_>,
    name: &str,
) -> Result<(String, RangeSpec), DslError> {
    parse_body(p, name, "date_range")
}

fn parse_body(p: &mut Parser<'_>, name: &str, agg: &str) -> Result<(String, RangeSpec), DslError> {
    let keys = if agg == "range" { KEYS } else { DATE_KEYS };
    let mut field: Option<String> = None;
    let mut spec = RangeSpec::default();
    // The bounds given as text, each with the milliseconds it parsed to.
    // ALLOC: one entry a text bound, bounded by the body's recording.
    let mut texts: Vec<(f64, String)> = Vec::new();
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
        if !keys.contains(&key) {
            return Err(DslError::xcontent(
                key_at,
                // ALLOC: error path; one message.
                &format!("[{agg}] unknown field [{key}]{}", suggest(key, keys)),
                None,
            ));
        }
        if !(matches!(key, "field" | "keyed" | "ranges") || key == "format" && agg == "date_range")
        {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [{agg}] aggregation's [{key}]"
            )));
        }
        let accepted = match key {
            "field" | "format" => kind == Kind::String,
            "keyed" => matches!(kind, Kind::Bool | Kind::String),
            _ => matches!(kind, Kind::StartArray | Kind::StartObject),
        };
        if !accepted {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[{agg}] {key} doesn't support values of type: {}",
                    kind.name()
                ),
                None,
            ));
        }
        match key {
            // ALLOC: one field name an aggregation, bounded by the body's recording.
            "field" => field = Some(p.text().to_owned()),
            "keyed" => {
                spec.keyed = bool_value(p).map_err(|cause| {
                    DslError::xcontent(
                        p.location(),
                        // ALLOC: error path; one message.
                        &format!("[{agg}] failed to parse field [keyed]"),
                        Some(cause),
                    )
                })?;
            }
            "format" => {
                let text = p.text();
                spec.format = Some(DatePattern::parse(text).ok_or_else(|| {
                    // ALLOC: error path; one message.
                    DslError::unsupported(&format!(
                        "the [date_range] aggregation's [format] of [{text}]"
                    ))
                })?);
            }
            _ => parse_ranges(p, kind, &mut spec.ranges, &mut texts, agg)?,
        }
    }
    check_texts(spec.format.as_ref(), &texts)?;
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    if spec.ranges.is_empty() {
        return Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("No [ranges] specified for the [{name}] aggregation"),
        ));
    }
    // `sortRanges`: a stable sort by `from`, then `to`, as `Double.compare`.
    spec.ranges.sort_by(|a, b| {
        a.from
            .total_cmp(&b.from)
            .then_with(|| a.to.total_cmp(&b.to))
    });
    Ok((field, spec))
}

/// The ranges of a `ranges` value: an array of range objects, or one object.
fn parse_ranges(
    p: &mut Parser<'_>,
    kind: Kind,
    out: &mut Vec<RangeDef>,
    texts: &mut Vec<(f64, String)>,
    agg: &str,
) -> Result<(), DslError> {
    let failed = |p: &Parser<'_>, cause: DslError| {
        DslError::xcontent(
            p.location(),
            // ALLOC: error path; one message.
            &format!("[{agg}] failed to parse field [ranges]"),
            Some(cause),
        )
    };
    if kind == Kind::StartObject {
        let def = parse_range(p, texts, agg).map_err(|e| failed(p, e))?;
        out.push(def);
        return Ok(());
    }
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => return Ok(()),
            Some(Kind::StartObject) => {
                let def = parse_range(p, texts, agg).map_err(|e| failed(p, e))?;
                // ALLOC: one range, bounded by the body's recording.
                out.push(def);
            }
            Some(other) => {
                return Err(failed(
                    p,
                    DslError::xcontent(
                        p.location(),
                        // ALLOC: error path; one message.
                        &format!(
                            "[{agg}] ranges doesn't support values of type: {}",
                            other.name()
                        ),
                        None,
                    ),
                ));
            }
        }
    }
}

/// One range object, the parser on its start: `key`, `from` and `to`, each
/// optional, `from` and `to` a number or null. A bound given as text is
/// parsed by the field's format, which is not served yet.
fn parse_range(
    p: &mut Parser<'_>,
    texts: &mut Vec<(f64, String)>,
    agg: &str,
) -> Result<RangeDef, DslError> {
    let mut def = RangeDef {
        key: None,
        from: f64::NEG_INFINITY,
        to: f64::INFINITY,
    };
    let mut key = "";
    let mut key_at = p.location();
    loop {
        let kind = match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(def),
            Some(Kind::FieldName) => {
                key = p.text();
                key_at = p.location();
                continue;
            }
            Some(kind) => kind,
        };
        if !RANGE_KEYS.contains(&key) {
            return Err(DslError::xcontent(
                key_at,
                // ALLOC: error path; one message.
                &format!("[{agg}] unknown field [{key}]{}", suggest(key, RANGE_KEYS)),
                None,
            ));
        }
        match (key, kind) {
            // `ValueType.DOUBLE` reads a string or a number as text.
            // ALLOC: one key a range, bounded by the body's recording.
            ("key", Kind::String | Kind::Number) => def.key = Some(p.text().to_owned()),
            ("from" | "to", Kind::Null) => {}
            ("from" | "to", Kind::Number) => {
                let value = double_value(kind, p.text()).map_err(|cause| {
                    DslError::xcontent(
                        p.location(),
                        // ALLOC: error path; one message.
                        &format!("[{agg}] failed to parse field [{key}]"),
                        Some(cause),
                    )
                })?;
                if key == "from" {
                    def.from = value;
                } else {
                    def.to = value;
                }
            }
            ("from" | "to", Kind::String) if agg == "date_range" => {
                let value = date_bound(p, key, agg)?;
                // ALLOC: one text a text bound, bounded by the body's recording.
                texts.push((value, p.text().to_owned()));
                if key == "from" {
                    def.from = value;
                } else {
                    def.to = value;
                }
            }
            ("from" | "to", Kind::String) => {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "a [{agg}]'s [{key}] given as text"
                )));
            }
            _ => {
                return Err(DslError::xcontent(
                    p.location(),
                    // ALLOC: error path; one message.
                    &format!(
                        "[{agg}] {key} doesn't support values of type: {}",
                        kind.name()
                    ),
                    None,
                ));
            }
        }
    }
}

/// A `date_range` bound given as text, the parser on it: a date in the
/// default format as epoch milliseconds. Date math (`now`, `||`) needs the
/// request's instant and is not served.
fn date_bound(p: &Parser<'_>, key: &str, agg: &str) -> Result<f64, DslError> {
    let text = p.text();
    match crate::date::parse_to_millis(text, false) {
        // ARITH: none; a date's milliseconds are far below 2^53.
        #[allow(clippy::cast_precision_loss)]
        Ok(millis) => Ok(millis as f64),
        Err(crate::date::DateError::Math) => Err(DslError::unsupported(
            // ALLOC: error path; one message.
            &format!("date math in the [{agg}] bound [{text}]"),
        )),
        Err(e) => Err(DslError::xcontent(
            p.location(),
            // ALLOC: error path; one message.
            &format!("[{agg}] failed to parse field [{key}]"),
            Some(DslError::Failure(e.into_failure(text))),
        )),
    }
}

/// A bound given as text is parsed by the aggregation's `format`, which this
/// node only prints: it is served when the format prints the parsed instant
/// as the very text given (the default parser then reads what the format
/// would), and refused otherwise.
fn check_texts(format: Option<&DatePattern>, texts: &[(f64, String)]) -> Result<(), DslError> {
    let Some(pattern) = format else {
        return Ok(());
    };
    for (millis, text) in texts {
        // ALLOC: one date text, bounded by the format.
        let mut printed = Vec::new();
        // ARITH: none; a parsed date's milliseconds are far below 2^63.
        #[allow(clippy::cast_possible_truncation)]
        let wrote = pattern.write(&mut printed, *millis as i64, 0);
        if wrote.is_err() || printed != text.as_bytes() {
            return Err(DslError::unsupported(
                // ALLOC: error path; one message.
                &format!("a date_range bound [{text}] that is not in the [format]'s own text"),
            ));
        }
    }
    Ok(())
}
