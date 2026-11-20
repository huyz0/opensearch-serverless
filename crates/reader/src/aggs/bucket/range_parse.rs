//! `range`'s body: `RangeAggregationBuilder`'s `ObjectParser`
//! (`ValuesSourceAggregationBuilder.declareFields` and its own `keyed` and
//! `ranges`) and `RangeAggregator.Range.PARSER`, at OpenSearch 3.10.0, and
//! `AbstractRangeBuilder.sortRanges`.

use crate::aggs::bucket::range::{RangeDef, RangeSpec};
use crate::aggs::metric::percentiles::double_value;
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

/// Every key a range declares.
const RANGE_KEYS: &[&str] = &["key", "from", "to"];

/// The field and options of a `range` body, the parser on its start.
pub fn parse_range_body(p: &mut Parser<'_>, name: &str) -> Result<(String, RangeSpec), DslError> {
    let mut field: Option<String> = None;
    let mut spec = RangeSpec::default();
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
                &format!("[range] unknown field [{key}]{}", suggest(key, KEYS)),
                None,
            ));
        }
        if !matches!(key, "field" | "keyed" | "ranges") {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [range] aggregation's [{key}]"
            )));
        }
        let accepted = match key {
            "field" => kind == Kind::String,
            "keyed" => matches!(kind, Kind::Bool | Kind::String),
            _ => matches!(kind, Kind::StartArray | Kind::StartObject),
        };
        if !accepted {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[range] {key} doesn't support values of type: {}",
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
                        "[range] failed to parse field [keyed]",
                        Some(cause),
                    )
                })?;
            }
            _ => parse_ranges(p, kind, &mut spec.ranges)?,
        }
    }
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
fn parse_ranges(p: &mut Parser<'_>, kind: Kind, out: &mut Vec<RangeDef>) -> Result<(), DslError> {
    let failed = |p: &Parser<'_>, cause: DslError| {
        DslError::xcontent(
            p.location(),
            "[range] failed to parse field [ranges]",
            Some(cause),
        )
    };
    if kind == Kind::StartObject {
        let def = parse_range(p).map_err(|e| failed(p, e))?;
        out.push(def);
        return Ok(());
    }
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => return Ok(()),
            Some(Kind::StartObject) => {
                let def = parse_range(p).map_err(|e| failed(p, e))?;
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
                            "[range] ranges doesn't support values of type: {}",
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
fn parse_range(p: &mut Parser<'_>) -> Result<RangeDef, DslError> {
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
                &format!("[range] unknown field [{key}]{}", suggest(key, RANGE_KEYS)),
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
                        &format!("[range] failed to parse field [{key}]"),
                        Some(cause),
                    )
                })?;
                if key == "from" {
                    def.from = value;
                } else {
                    def.to = value;
                }
            }
            ("from" | "to", Kind::String) => {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "a [range]'s [{key}] given as text"
                )));
            }
            _ => {
                return Err(DslError::xcontent(
                    p.location(),
                    // ALLOC: error path; one message.
                    &format!(
                        "[range] {key} doesn't support values of type: {}",
                        kind.name()
                    ),
                    None,
                ));
            }
        }
    }
}
