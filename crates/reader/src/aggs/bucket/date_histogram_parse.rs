//! `date_histogram`'s body: `DateHistogramAggregationBuilder`'s
//! `ObjectParser` (`ValuesSourceAggregationBuilder.declareFields`,
//! `DateIntervalWrapper` and `LongBounds`), its setters' checks and
//! `innerBuild`'s rounding, at OpenSearch 3.10.0.

use crate::aggs::bucket::date_histogram::{
    DateHistogramSpec, DateRounding, DateUnit, RoundingKind, round_bound,
};
use crate::aggs::bucket::histogram::HistogramOrder;
use crate::aggs::bucket::histogram_parse::parse_order;
use crate::aggs::metric::parse::long_value;
use crate::date::parse_to_millis;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, suggest};

/// Every key `date_histogram` declares.
const KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "format",
    "script",
    "time_zone",
    "interval",
    "calendar_interval",
    "fixed_interval",
    "offset",
    "keyed",
    "min_doc_count",
    "extended_bounds",
    "hard_bounds",
    "order",
];

/// `DateHistogramAggregationBuilder.DATE_FIELD_UNITS`.
pub(crate) fn calendar_unit(text: &str) -> Option<DateUnit> {
    Some(match text {
        "year" | "1y" => DateUnit::Year,
        "quarter" | "1q" => DateUnit::Quarter,
        "month" | "1M" => DateUnit::Month,
        "week" | "1w" => DateUnit::Week,
        "day" | "1d" => DateUnit::Day,
        "hour" | "1h" => DateUnit::Hour,
        "minute" | "1m" => DateUnit::Minute,
        "second" | "1s" => DateUnit::Second,
        _ => return None,
    })
}

/// `TimeValue.parseTimeValue(text, null, setting).millis()`, or the message
/// of the `IllegalArgumentException` it throws.
pub fn time_value_millis(text: &str, setting: &str) -> Result<i64, String> {
    let normalized = text.to_lowercase();
    let normalized = normalized.trim();
    // `m` is case-sensitive: `M` is months, which is no time value.
    let units: [(&str, &str, i64); 7] = [
        ("nanos", "nanos", 1),
        ("micros", "micros", 2),
        ("ms", "ms", 3),
        ("s", "s", 4),
        ("m", "m", 5),
        ("h", "h", 6),
        ("d", "d", 7),
    ];
    for (suffix, _, kind) in units {
        let matches = if kind == 5 {
            text.ends_with('m')
        } else {
            normalized.ends_with(suffix)
        };
        // `ms` and `s` overlap: the first match in the list's order wins, as
        // the Java's chain does.
        if !matches {
            continue;
        }
        let digits = normalized.strip_suffix(suffix).unwrap_or(normalized).trim();
        let value: i64 = match digits.parse() {
            Ok(v) => v,
            Err(_) if digits.parse::<f64>().is_ok() => {
                // ALLOC: error path; one message.
                return Err(format!(
                    "failed to parse [{text}], fractional time values are not supported"
                ));
            }
            // ALLOC: error path; one message.
            Err(_) => return Err(format!("failed to parse [{text}]")),
        };
        if value < -1 {
            // ALLOC: error path; one message.
            return Err(format!(
                "failed to parse setting [{setting}] with value [{text}] as a time value: \
                 negative durations are not supported"
            ));
        }
        return Ok(match kind {
            1 => value / 1_000_000,
            2 => value / 1_000,
            3 => value,
            4 => value.saturating_mul(1_000),
            5 => value.saturating_mul(60_000),
            6 => value.saturating_mul(3_600_000),
            _ => value.saturating_mul(86_400_000),
        });
    }
    let all_zero = !normalized.is_empty() && normalized.bytes().all(|b| b == b'0');
    let minus_one = normalized
        .strip_prefix('-')
        .is_some_and(|d| !d.is_empty() && d.trim_start_matches('0') == "1");
    if minus_one {
        return Ok(-1);
    }
    if all_zero {
        return Ok(0);
    }
    // ALLOC: error path; one message.
    Err(format!(
        "failed to parse setting [{setting}] with value [{text}] as a time value: unit is \
         missing or unrecognized"
    ))
}

/// A fixed-offset zone's offset in milliseconds, for the ids `ZoneId.of`
/// reads as one: `Z`, `UTC`, `GMT`, `UT`, and `+h`, `+hh`, `+hh:mm`,
/// `+hhmm`, `+hh:mm:ss` or `+hhmmss`, alone or after `UTC`, `GMT` or `UT`.
pub(crate) fn zone_millis(id: &str) -> Option<i64> {
    if matches!(id, "Z" | "UTC" | "GMT" | "UT") {
        return Some(0);
    }
    let rest = ["UTC", "GMT", "UT"]
        .iter()
        .find_map(|p| id.strip_prefix(p))
        .unwrap_or(id);
    let sign = match rest.as_bytes().first()? {
        b'+' => 1i64,
        b'-' => -1i64,
        _ => return None,
    };
    let body = rest.get(1..)?;
    // ALLOC: at most three short slices.
    let parts: Vec<&str> = if body.contains(':') {
        body.split(':').collect()
    } else if body.len() <= 2 {
        // ALLOC: one slice.
        vec![body]
    } else if body.len() == 4 || body.len() == 6 {
        body.as_bytes()
            .chunks(2)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            // ALLOC: at most three short slices.
            .collect()
    } else {
        return None;
    };
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut seconds = 0i64;
    for (i, part) in parts.iter().enumerate() {
        let ok_len = if i == 0 {
            (1..=2).contains(&part.len())
        } else {
            part.len() == 2
        };
        if !ok_len || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: i64 = part.parse().ok()?;
        if i > 0 && n > 59 {
            return None;
        }
        seconds = seconds.saturating_add(n.saturating_mul([3600, 60, 1][i]));
    }
    if seconds > 18 * 3600 {
        return None;
    }
    Some(sign.saturating_mul(seconds).saturating_mul(1000))
}

/// One bound as the request gave it: epoch milliseconds, or a date.
struct Bound {
    millis: i64,
    /// `LongBounds.toString`'s text for it.
    shown: String,
}

/// `LongBounds.PARSER` over an object, the parser on its start: each end
/// null (`None`), a number or a date.
fn parse_bounds(p: &mut Parser<'_>) -> Result<(Option<Bound>, Option<Bound>), DslError> {
    let mut ends: [Option<Bound>; 2] = [None, None];
    let mut at = 0usize;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                at = match p.text() {
                    "min" => 0,
                    "max" => 1,
                    other => {
                        return Err(DslError::illegal(
                            // ALLOC: error path; one message.
                            format!("[bounds] unknown field [{other}]"),
                        ));
                    }
                };
            }
            Some(Kind::Null) => {}
            Some(kind @ (Kind::Number | Kind::String)) => {
                let bound = if kind == Kind::Number {
                    let millis = long_value(kind, p.text())?;
                    Bound {
                        millis,
                        // ALLOC: error path text, one short string a bound.
                        shown: millis.to_string(),
                    }
                } else {
                    let text = p.text();
                    let millis = parse_to_millis(text, false)
                        .map_err(|e| DslError::Failure(e.into_failure(text)))?;
                    Bound {
                        millis,
                        // ALLOC: as above.
                        shown: text.to_owned(),
                    }
                };
                if let Some(slot) = ends.get_mut(at) {
                    *slot = Some(bound);
                }
            }
            Some(_) => {
                return Err(DslError::illegal("Unknown field type of a bound"));
            }
        }
    }
    let [min, max] = ends;
    Ok((min, max))
}

fn shown(bound: &Option<Bound>) -> &str {
    bound.as_ref().map_or("", |b| b.shown.as_str())
}

/// The field and options of a `date_histogram` body, the parser on its
/// start.
pub fn parse_date_histogram_body(
    p: &mut Parser<'_>,
    name: &str,
) -> Result<(String, DateHistogramSpec), DslError> {
    let mut field: Option<String> = None;
    let mut zone_ms = 0i64;
    let mut kind: Option<RoundingKind> = None;
    let mut interval_key: Option<&'static str> = None;
    let mut offset = 0i64;
    let mut keyed = false;
    let mut min_doc_count = 0u64;
    let mut order = HistogramOrder::KeyAsc;
    let mut extended: Option<(Option<Bound>, Option<Bound>)> = None;
    let mut hard: Option<(Option<Bound>, Option<Bound>)> = None;
    let mut key = "";
    let mut key_at = p.location();
    loop {
        let token = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                key_at = p.location();
                continue;
            }
            Some(token) => token,
        };
        if !KEYS.contains(&key) {
            return Err(DslError::xcontent(
                key_at,
                // ALLOC: error path; one message.
                &format!(
                    "[date_histogram] unknown field [{key}]{}",
                    suggest(key, KEYS)
                ),
                None,
            ));
        }
        if matches!(
            key,
            "missing" | "value_type" | "format" | "script" | "interval"
        ) {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [date_histogram] aggregation's [{key}]"
            )));
        }
        let accepted = match key {
            "field" | "calendar_interval" | "fixed_interval" => token == Kind::String,
            "keyed" => matches!(token, Kind::Bool | Kind::String),
            "extended_bounds" | "hard_bounds" => token == Kind::StartObject,
            "order" => matches!(token, Kind::StartObject | Kind::StartArray),
            _ => matches!(token, Kind::Number | Kind::String),
        };
        if !accepted {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[date_histogram] {key} doesn't support values of type: {}",
                    token.name()
                ),
                None,
            ));
        }
        let failed = |p: &Parser<'_>, cause: DslError| {
            DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!("[date_histogram] failed to parse field [{key}]"),
                Some(cause),
            )
        };
        match key {
            // ALLOC: one field name an aggregation, bounded by the body's recording.
            "field" => field = Some(p.text().to_owned()),
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
                if text.is_empty() {
                    return Err(failed(
                        p,
                        DslError::illegal("[interval] must not be null: [date_histogram]"),
                    ));
                }
                kind = Some(if this == "calendar_interval" {
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
                    RoundingKind::Interval(ms)
                });
            }
            "time_zone" => {
                zone_ms = if token == Kind::Number {
                    let hours = long_value(token, p.text())?;
                    hours.saturating_mul(3_600_000)
                } else {
                    zone_millis(p.text()).ok_or_else(|| {
                        // ALLOC: error path; one message.
                        DslError::unsupported(&format!(
                            "the [date_histogram] aggregation's time zone [{}]",
                            p.text()
                        ))
                    })?
                };
            }
            "offset" => {
                offset = if token == Kind::Number {
                    long_value(token, p.text()).map_err(|e| failed(p, e))?
                } else {
                    offset_millis(p.text()).map_err(|m| failed(p, DslError::illegal(m)))?
                };
            }
            "keyed" => keyed = bool_value(p).map_err(|e| failed(p, e))?,
            "min_doc_count" => {
                let n = long_value(token, p.text()).map_err(|e| failed(p, e))?;
                min_doc_count = u64::try_from(n).map_err(|_| {
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
            "extended_bounds" => extended = Some(parse_bounds(p).map_err(|e| failed(p, e))?),
            "hard_bounds" => hard = Some(parse_bounds(p).map_err(|e| failed(p, e))?),
            _ => order = parse_order(p, token, "date_histogram")?,
        }
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    let Some(kind) = kind else {
        return Err(DslError::illegal(
            "Invalid interval specified, must be non-null and non-empty",
        ));
    };
    if let RoundingKind::Interval(ms) = kind
        && ms < 1
    {
        return Err(DslError::illegal(
            "Zero or negative time interval not supported",
        ));
    }
    let rounding = DateRounding {
        kind,
        zone_ms,
        offset,
    };
    let extended_bounds = check_and_round(name, "extended_bounds", extended.as_ref(), &rounding)?;
    let hard_bounds = check_and_round(name, "hard_bounds", hard.as_ref(), &rounding)?;
    if let (Some((ext_min, ext_max)), Some((hard_min, hard_max))) = (extended_bounds, hard_bounds) {
        let outside = matches!((ext_max, hard_max), (Some(e), Some(h)) if e > h)
            || matches!((ext_min, hard_min), (Some(e), Some(h)) if e < h);
        if outside {
            let (hs, es) = (hard.as_ref(), extended.as_ref());
            return Err(DslError::illegal(
                // ALLOC: error path; one message.
                format!(
                    "Extended bounds have to be inside hard bounds, hard bounds: [{}--{}], \
                     extended bounds: [{}--{}]",
                    hs.map_or("", |b| shown(&b.0)),
                    hs.map_or("", |b| shown(&b.1)),
                    es.map_or("", |b| shown(&b.0)),
                    es.map_or("", |b| shown(&b.1)),
                ),
            ));
        }
    }
    Ok((
        field,
        DateHistogramSpec {
            rounding,
            keyed,
            min_doc_count,
            order,
            extended_bounds,
            hard_bounds: hard_bounds.unwrap_or((None, None)),
        },
    ))
}

/// `DateHistogramAggregationBuilder.parseStringOffset`: a signed time value.
pub(crate) fn offset_millis(text: &str) -> Result<i64, String> {
    let setting = "DateHistogramAggregationBuilder.parseOffset";
    match text.strip_prefix('-') {
        Some(rest) => time_value_millis(rest, setting).map(i64::wrapping_neg),
        None => time_value_millis(text.strip_prefix('+').unwrap_or(text), setting),
    }
}

/// `LongBounds.parseAndValidate` then `round`: `None` when none were given.
#[allow(clippy::type_complexity)]
fn check_and_round(
    agg: &str,
    which: &str,
    bounds: Option<&(Option<Bound>, Option<Bound>)>,
    rounding: &DateRounding,
) -> Result<Option<(Option<i64>, Option<i64>)>, DslError> {
    let Some((min, max)) = bounds else {
        return Ok(None);
    };
    let (lo, hi) = (
        min.as_ref().map(|b| b.millis),
        max.as_ref().map(|b| b.millis),
    );
    if let (Some(lo), Some(hi)) = (lo, hi)
        && lo > hi
    {
        return Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!(
                "[{which}.min][{lo}] cannot be greater than [{which}.max][{hi}] for histogram \
                 aggregation [{agg}]"
            ),
        ));
    }
    Ok(Some((
        lo.map(|v| round_bound(rounding, v)),
        hi.map(|v| round_bound(rounding, v)),
    )))
}
