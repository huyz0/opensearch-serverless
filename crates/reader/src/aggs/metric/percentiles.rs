//! `percentiles` and `percentile_ranks` of a numeric or date field: the
//! `ConstructingObjectParser` of `AbstractPercentilesAggregationBuilder`
//! and its two builders, and `AbstractInternalTDigestPercentiles`'
//! response body, at OpenSearch 3.10.0
//! (server/src/main/java/org/opensearch/search/aggregations/metrics/).
//!
//! Only the t-digest method is served; `hdr`, `missing`, `value_type`,
//! `format` and `script` are refused as not served yet.

use std::io::{self, Write};

use crate::aggs::metric::Metric;
use crate::aggs::metric::date_text::write_date_of_double;
use crate::aggs::metric::double::write_java_double;
use crate::aggs::metric::tdigest::Digest;
use crate::aggs::partial::Format;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, suggest};
use crate::failure::Failure;
use oss_rest::json::Location;

/// `PercentilesAggregationBuilder.DEFAULT_PERCENTS`.
pub const DEFAULT_PERCENTS: [f64; 7] = [1.0, 5.0, 25.0, 50.0, 75.0, 95.0, 99.0];

/// `PercentilesConfig.TDigest`'s default compression.
pub const DEFAULT_COMPRESSION: f64 = 100.0;

/// The largest compression served: a digest of it holds about 8 MB while
/// it is built, and the engine keeps its sort keys, about 5 MB, for the
/// thread's next digest. Far above it OpenSearch fails or exhausts its heap.
pub const MAX_COMPRESSION: f64 = 10_000.0;

/// What a `percentiles` or `percentile_ranks` aggregation asks for besides
/// its field.
#[derive(Clone, Debug, PartialEq)]
pub struct PercentilesOptions {
    /// The percents (`percents`) or the values ranked (`values`), sorted as
    /// `Arrays.sort` sorts them, duplicates kept.
    pub keys: Vec<f64>,
    /// `keyed`: the response's values as an object rather than an array.
    pub keyed: bool,
    /// `tdigest.compression`.
    pub compression: f64,
}

impl Default for PercentilesOptions {
    fn default() -> Self {
        PercentilesOptions {
            // ALLOC: empty.
            keys: Vec::new(),
            keyed: true,
            compression: DEFAULT_COMPRESSION,
        }
    }
}

/// The keys the builders declare: `declareFields(parser, true, true,
/// false)` and their own.
const PERCENTILES_KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "format",
    "script",
    "percents",
    "keyed",
    "tdigest",
    "hdr",
];

/// As [`PERCENTILES_KEYS`], with `values` for `percents`.
const RANKS_KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "format",
    "script",
    "values",
    "keyed",
    "tdigest",
    "hdr",
];

/// `Double.parseDouble(text)`: surrounding spaces and controls ignored, a
/// trailing `d` or `f` allowed, `NaN` and `Infinity` signed or not. A
/// hexadecimal float is not read.
pub fn java_parse_double(text: &str) -> Option<f64> {
    let t = text.trim_matches(|c: char| c <= ' ');
    let (sign, body) = match t.as_bytes().first() {
        Some(b'-') => (-1.0, t.get(1..).unwrap_or_default()),
        Some(b'+') => (1.0, t.get(1..).unwrap_or_default()),
        _ => (1.0, t),
    };
    match body {
        "NaN" => return Some(f64::NAN),
        "Infinity" => return Some(sign * f64::INFINITY),
        _ => {}
    }
    let digits = body
        .strip_suffix(['d', 'D', 'f', 'F'])
        .filter(|d| d.ends_with(|c: char| c.is_ascii_digit() || c == '.'))
        .unwrap_or(body);
    if digits.is_empty()
        || !digits
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return None;
    }
    digits.parse::<f64>().ok().map(|v| sign * v)
}

/// `parser.doubleValue()`, the parser on a number or a string.
pub(crate) fn double_value(kind: Kind, text: &str) -> Result<f64, DslError> {
    let value = match kind {
        Kind::Number => text.parse::<f64>().ok(),
        Kind::String => java_parse_double(text),
        _ => {
            return Err(DslError::Failure(Failure::plain(
                "input_coercion_exception",
                // ALLOC: error path; one message.
                format!(
                    "Current token ({}) not numeric, can not use numeric value accessors",
                    kind.name()
                ),
            )));
        }
    };
    value.ok_or_else(|| {
        DslError::Failure(Failure::plain(
            "number_format_exception",
            // ALLOC: error path; one message.
            format!("For input string: \"{text}\""),
        ))
    })
}

/// `"[<ty>] failed to parse field [<key>]"` around `cause`, where the parser is.
fn failed(p: &Parser<'_>, ty: &str, key: &str, cause: DslError) -> DslError {
    failed_at(p.location(), ty, key, cause)
}

/// As [`failed`], located at `at`.
fn failed_at(at: Location, ty: &str, key: &str, cause: DslError) -> DslError {
    DslError::xcontent(
        at,
        // ALLOC: error path; one message.
        &format!("[{ty}] failed to parse field [{key}]"),
        Some(cause),
    )
}

/// `"[<ty>] <key> doesn't support values of type: <kind>"`.
fn wrong_type(p: &Parser<'_>, ty: &str, key: &str, kind: Kind) -> DslError {
    DslError::xcontent(
        p.location(),
        // ALLOC: error path; one message.
        &format!(
            "[{ty}] {key} doesn't support values of type: {}",
            kind.name()
        ),
        None,
    )
}

/// `declareDoubleArray`'s values, the parser on an array or one value.
fn parse_doubles(
    p: &mut Parser<'_>,
    kind: Kind,
    ty: &str,
    key: &str,
) -> Result<Vec<f64>, DslError> {
    // ALLOC: one double a value, bounded by the body's recording.
    let mut values = Vec::new();
    if kind != Kind::StartArray {
        let v = double_value(kind, p.text()).map_err(|e| failed(p, ty, key, e))?;
        values.push(v);
        return Ok(values);
    }
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => return Ok(values),
            Some(item) => {
                let v = double_value(item, p.text()).map_err(|e| failed(p, ty, key, e))?;
                values.push(v);
            }
        }
    }
}

/// `PercentilesMethod.TDIGEST_PARSER` over its object: the compression.
fn parse_tdigest(p: &mut Parser<'_>, ty: &str) -> Result<f64, DslError> {
    let mut compression = DEFAULT_COMPRESSION;
    let mut key = "";
    let mut key_at = p.location();
    loop {
        let kind = match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(compression),
            Some(Kind::FieldName) => {
                key = p.text();
                key_at = p.location();
                continue;
            }
            Some(kind) => kind,
        };
        if key != "compression" {
            let unknown = DslError::xcontent(
                key_at,
                // ALLOC: error path; one message.
                &format!(
                    "[tdigest] unknown field [{key}]{}",
                    suggest(key, &["compression"])
                ),
                None,
            );
            return Err(failed(p, ty, "tdigest", unknown));
        }
        let inner = if !matches!(kind, Kind::Number | Kind::String) {
            wrong_type(p, "tdigest", key, kind)
        } else {
            match double_value(kind, p.text()) {
                Ok(c) if c < 0.0 => failed(
                    p,
                    "tdigest",
                    key,
                    // ALLOC: error path; one message.
                    DslError::illegal(format!(
                        "[compression] must be greater than or equal to 0. Found [{}]",
                        java_double_text(c)
                    )),
                ),
                Ok(c) if c.is_nan() || c > MAX_COMPRESSION => {
                    // ALLOC: error path; one message.
                    return Err(DslError::unsupported(&format!(
                        "a [tdigest] compression of [{}]",
                        java_double_text(c)
                    )));
                }
                Ok(c) => {
                    compression = c;
                    continue;
                }
                Err(e) => failed(p, "tdigest", key, e),
            }
        };
        return Err(failed(p, ty, "tdigest", inner));
    }
}

/// `Double.toString(d)` as a string, for messages.
fn java_double_text(d: f64) -> String {
    // ALLOC: one short number's text.
    let mut buf = Vec::new();
    // Writing to a vector cannot fail.
    let _ = write_java_double(&mut buf, d);
    String::from_utf8(buf).unwrap_or_default()
}

/// The builders' parser over the aggregation's object, the parser on its
/// start: the field and the options. `name` is the aggregation's name.
pub fn parse_percentiles_body(
    p: &mut Parser<'_>,
    metric: Metric,
    name: &str,
) -> Result<(String, PercentilesOptions), DslError> {
    let ty = metric.name();
    let ranks = metric == Metric::PercentileRanks;
    let (keys, values_key) = if ranks {
        (RANKS_KEYS, "values")
    } else {
        (PERCENTILES_KEYS, "percents")
    };
    let mut field: Option<String> = None;
    let mut options = PercentilesOptions::default();
    let mut points: Option<Vec<f64>> = None;
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
            "keyed" => matches!(kind, Kind::Bool | Kind::String),
            "tdigest" => kind == Kind::StartObject,
            k if k == values_key => {
                matches!(kind, Kind::StartArray | Kind::Number | Kind::String)
            }
            _ => {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "the [{ty}] aggregation's [{key}]"
                )));
            }
        };
        if !accepted {
            return Err(wrong_type(p, ty, key, kind));
        }
        match key {
            // ALLOC: one field name an aggregation, bounded by the body's recording.
            "field" => field = Some(p.text().to_owned()),
            "keyed" => options.keyed = bool_value(p).map_err(|e| failed(p, ty, key, e))?,
            "tdigest" => options.compression = parse_tdigest(p, ty)?,
            _ => points = Some(parse_doubles(p, kind, ty, key)?),
        }
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    options.keys = build_keys(points, ranks, values_key, name).map_err(|cause| {
        DslError::Failure(
            Failure::plain(
                "x_content_parse_exception",
                // ALLOC: error path; one message.
                format!("Failed to build [{ty}] after last required field arrived"),
            )
            .caused_by(cause.into_failure()),
        )
    })?;
    if ranks {
        // `MergingDigest.cdf`'s refusal, which OpenSearch reaches only as it
        // writes the response.
        if let Some(bad) = options.keys.iter().find(|v| !v.is_finite()) {
            // ALLOC: error path; one message.
            return Err(DslError::illegal(format!(
                "Invalid value: {}",
                java_double_text(*bad)
            )));
        }
    }
    Ok((field, options))
}

/// The builders' constructors: the keys checked and sorted, the default
/// percents when a `percentiles` names none.
fn build_keys(
    points: Option<Vec<f64>>,
    ranks: bool,
    values_key: &str,
    name: &str,
) -> Result<Vec<f64>, DslError> {
    let Some(mut points) = points else {
        if ranks {
            // ALLOC: error path; one message.
            return Err(DslError::illegal(format!(
                "[{values_key}] must not be null: [{name}]"
            )));
        }
        // ALLOC: seven defaults, a constant.
        return Ok(DEFAULT_PERCENTS.to_vec());
    };
    if points.is_empty() {
        let reason = if ranks {
            // ALLOC: error path; one message.
            format!("[{values_key}] must not be an empty array: [{name}]")
        } else {
            // ALLOC: error path; one message.
            format!("[{values_key}] must not be empty: [{name}]")
        };
        return Err(DslError::illegal(reason));
    }
    // `Arrays.sort(double[])`: -0.0 before 0.0, NaN last.
    points.sort_by(|a, b| {
        let canonical = |x: f64| if x.is_nan() { f64::NAN.abs() } else { x };
        canonical(*a).total_cmp(&canonical(*b))
    });
    if !ranks && let Some(bad) = points.iter().find(|p| **p < 0.0 || **p > 100.0) {
        // ALLOC: error path; one message.
        return Err(DslError::illegal(format!(
            "percent must be in [0,100], got [{}]: [{name}]",
            java_double_text(*bad)
        )));
    }
    Ok(points)
}

/// A double as `XContentBuilder` writes it: a number, or a string for NaN
/// and the infinities.
fn write_number(out: &mut dyn Write, d: f64) -> io::Result<()> {
    if d.is_finite() {
        write_java_double(out, d)
    } else {
        out.write_all(b"\"")?;
        write_java_double(out, d)?;
        out.write_all(b"\"")
    }
}

/// `AbstractInternalTDigestPercentiles.doXContentBody`: each key's
/// percentile (or rank, for `ranks`) of `digest`, `null` for an empty one.
/// `digest` must be settled ([`Digest::settle`]).
pub fn write_percentiles(
    out: &mut dyn Write,
    format: Format,
    ranks: bool,
    keys: &[f64],
    keyed: bool,
    digest: Option<&Digest>,
) -> io::Result<()> {
    let digest = digest.filter(|d| d.size() > 0);
    let dated = format == Format::Date && digest.is_some();
    out.write_all(if keyed {
        b"\"values\":{"
    } else {
        b"\"values\":["
    })?;
    for (i, &key) in keys.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        let value = digest.map(|d| value_of(d, ranks, key));
        if keyed {
            out.write_all(b"\"")?;
            write_java_double(out, key)?;
            out.write_all(b"\":")?;
        } else {
            out.write_all(b"{\"key\":")?;
            write_number(out, key)?;
            out.write_all(b",\"value\":")?;
        }
        match value {
            Some(v) => write_number(out, v)?,
            None => out.write_all(b"null")?,
        }
        if let (true, Some(v)) = (dated, value) {
            if keyed {
                out.write_all(b",\"")?;
                write_java_double(out, key)?;
                out.write_all(b"_as_string\":\"")?;
            } else {
                out.write_all(b",\"value_as_string\":\"")?;
            }
            write_date_of_double(out, v)?;
            out.write_all(b"\"")?;
        }
        if !keyed {
            out.write_all(b"}")?;
        }
    }
    out.write_all(if keyed { b"}" } else { b"]" })
}

/// `InternalTDigestPercentiles.percentile` or
/// `InternalTDigestPercentileRanks.percentileRank` of `key`.
fn value_of(digest: &Digest, ranks: bool, key: f64) -> f64 {
    if !ranks {
        return digest.quantile(key / 100.0);
    }
    // Clamped to [0, 1]; NaN stays NaN, as in Java.
    digest.cdf(key).clamp(0.0, 1.0) * 100.0
}

#[cfg(test)]
#[path = "percentiles_tests.rs"]
mod tests;
