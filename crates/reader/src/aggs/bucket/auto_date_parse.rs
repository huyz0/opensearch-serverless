//! `auto_date_histogram`'s body: `AutoDateHistogramAggregationBuilder.PARSER`
//! (`ValuesSourceAggregationBuilder.declareFields`) and its setters' checks,
//! at OpenSearch 3.10.0.

use crate::aggs::bucket::auto_date::{AutoDateSpec, STEPS};
use crate::aggs::bucket::date_histogram_zone::zone_of;
use crate::aggs::metric::parse::long_value;
use crate::date_pattern::DatePattern;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::suggest;

/// Every key `auto_date_histogram` declares.
const KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "format",
    "script",
    "time_zone",
    "buckets",
    "minimum_interval",
];

/// The `minimum_interval`s the builder allows, as it lists them, and the
/// index in the rounding list each starts at.
const INTERVALS: [(&str, usize); 6] = [
    ("minute", 1),
    ("day", 3),
    ("second", 0),
    ("year", 5),
    ("hour", 2),
    ("month", 4),
];

/// The field and options of an `auto_date_histogram` body, the parser on its
/// start; `name` is the aggregation's.
pub fn parse_auto_date_body(
    p: &mut Parser<'_>,
    name: &str,
) -> Result<(String, AutoDateSpec), DslError> {
    let mut field: Option<String> = None;
    let mut target = 10usize;
    let mut base = 0usize;
    let (mut zone_ms, mut region) = (0i64, None);
    let mut format = None;
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
                    "[auto_date_histogram] unknown field [{key}]{}",
                    suggest(key, KEYS)
                ),
                None,
            ));
        }
        if matches!(key, "missing" | "value_type" | "script") {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [auto_date_histogram] aggregation's [{key}]"
            )));
        }
        let accepted = match key {
            "field" | "format" | "minimum_interval" => token == Kind::String,
            _ => matches!(token, Kind::Number | Kind::String),
        };
        if !accepted {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[auto_date_histogram] {key} doesn't support values of type: {}",
                    token.name()
                ),
                None,
            ));
        }
        let failed = |p: &Parser<'_>, cause: DslError| {
            DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!("[auto_date_histogram] failed to parse field [{key}]"),
                Some(cause),
            )
        };
        match key {
            // ALLOC: one field name an aggregation, bounded by the body's recording.
            "field" => field = Some(p.text().to_owned()),
            "buckets" => {
                let n = long_value(token, p.text()).map_err(|e| failed(p, e))?;
                target = usize::try_from(n).ok().filter(|&n| n > 0).ok_or_else(|| {
                    failed(
                        p,
                        DslError::illegal(
                            // ALLOC: error path; one message.
                            format!("buckets must be greater than 0 for [{name}]"),
                        ),
                    )
                })?;
            }
            "minimum_interval" => {
                let text = p.text();
                base = INTERVALS
                    .iter()
                    .find(|(unit, _)| *unit == text)
                    .map(|&(_, at)| at)
                    .ok_or_else(|| {
                        failed(
                            p,
                            DslError::illegal(
                                // ALLOC: error path; one message.
                                format!(
                                    "minimum_interval must be one of [{:?}]",
                                    INTERVALS.map(|(unit, _)| unit)
                                )
                                .replace('"', ""),
                            ),
                        )
                    })?;
            }
            "time_zone" => {
                (zone_ms, region) = if token == Kind::Number {
                    (long_value(token, p.text())?.saturating_mul(3_600_000), None)
                } else {
                    zone_of(p.text()).ok_or_else(|| {
                        // ALLOC: error path; one message.
                        DslError::unsupported(&format!(
                            "the [auto_date_histogram] aggregation's time zone [{}]",
                            p.text()
                        ))
                    })?
                };
            }
            _ => {
                let text = p.text();
                format = Some(DatePattern::parse(text).ok_or_else(|| {
                    // ALLOC: error path; one message.
                    DslError::unsupported(&format!(
                        "the [auto_date_histogram] aggregation's [format] of [{text}]"
                    ))
                })?);
            }
        }
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    debug_assert!(base < STEPS.len());
    Ok((
        field,
        AutoDateSpec {
            target,
            base,
            zone_ms,
            region,
            format,
        },
    ))
}
