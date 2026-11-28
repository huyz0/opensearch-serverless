//! The bodies of `geo_centroid` and `geo_bounds`: `ValuesSourceAggregationBuilder.declareFields`'
//! `field`, `missing` and `value_type` (and `script` on a `geo_centroid`), and a `geo_bounds`'
//! `wrap_longitude`. A field is all this node serves of them.

use crate::aggs::metric::Metric;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::suggest;

const CENTROID_KEYS: &[&str] = &["field", "missing", "value_type", "script"];
const BOUNDS_KEYS: &[&str] = &["field", "missing", "value_type", "wrap_longitude"];

/// The field and `wrap_longitude` of `metric`'s body, the parser on its start.
pub(super) fn parse_body(p: &mut Parser<'_>, metric: Metric) -> Result<(String, bool), DslError> {
    let ty = metric.name();
    let keys = if metric == Metric::GeoBounds {
        BOUNDS_KEYS
    } else {
        CENTROID_KEYS
    };
    let mut field: Option<String> = None;
    let mut wrap = true;
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
                &format!("[{ty}] unknown field [{key}]{}", suggest(key, keys)),
                None,
            ));
        }
        let accepted = match key {
            "field" => kind == Kind::String,
            "wrap_longitude" => matches!(kind, Kind::Bool | Kind::String),
            _ => {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "the [{ty}] aggregation's [{key}]"
                )));
            }
        };
        if !accepted {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[{ty}] {key} doesn't support values of type: {}",
                    kind.name()
                ),
                None,
            ));
        }
        match key {
            // ALLOC: one field name an aggregation, bounded by the body's recording.
            "field" => field = Some(p.text().to_owned()),
            _ => match p.text() {
                "true" => wrap = true,
                "false" => wrap = false,
                other => {
                    return Err(DslError::xcontent(
                        p.location(),
                        // ALLOC: error path; one message.
                        &format!("[{ty}] failed to parse field [{key}]"),
                        Some(DslError::illegal(format!(
                            "Failed to parse value [{other}] as only [true] or [false] are allowed."
                        ))),
                    ));
                }
            },
        }
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    Ok((field, wrap))
}
