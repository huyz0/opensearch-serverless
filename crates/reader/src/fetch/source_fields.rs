//! `fields`: each hit's values of the fields asked for, read from its
//! `_source`.
//!
//! Ported from `FieldFetcher`, `FetchFieldsPhase`, `SourceValueFetcher`,
//! `XContentMapValues.extractValue` and the `valueFetcher` of the keyword,
//! text, number and date mappers (server/src/main/java/org/opensearch/), at
//! the OpenSearch tip the Java shell builds on. A field's values are looked up
//! at its source path, nested arrays are flattened breadth first, and each
//! value is read as its mapper reads it. The hit's `fields` object comes out
//! in `java.util.HashMap` order.

use std::collections::VecDeque;
use std::io::{self, Write};

use oss_rest::write_json_str;

use super::docvalue_fields::{DocValueField, HitField, ValueKind};
use super::source_filter::scratch_bytes;
use super::source_map::{Value, java_hash_order, read};
use crate::aggs::metric::date_text::write_date;
use crate::date::parse_to_millis;
use crate::dsl::queries::build::simple_match;
use crate::dsl::tokens::{Kind, Recorder};
use crate::failure::Failure;
use crate::mapping::{FieldType, Mapping};
use crate::search::ShardError;

/// How a field's source values are read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceType {
    /// `keyword`: the value's text, unless longer than `ignore_above`.
    Keyword(Option<i64>),
    /// `text`: the value's text.
    Text,
    /// `byte`, `short` or `integer`, with the type's range.
    Int(i64, i64),
    /// `long`.
    Long,
    /// `double`: a finite double.
    Double,
    /// `float`: a finite float.
    Float,
    /// `half_float`: a float that is finite as a half.
    HalfFloat,
    /// `scaled_float` with this scaling factor (its bits): the value rounded at the factor.
    Scaled(u64),
    /// `date` in the default format.
    Date,
}

/// Where a `fields` entry's values sit and how they read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFetch {
    /// The dotted path in `_source`.
    pub path: String,
    /// How a value reads.
    pub ty: SourceType,
}

fn bad_request(reason: String) -> ShardError {
    ShardError {
        status: 400,
        failure: Failure::plain("illegal_argument_exception", reason),
    }
}

fn unsupported(what: &str) -> ShardError {
    // ALLOC: error path; one message.
    bad_request(format!("{what} is not supported by this node yet"))
}

/// What the `fields` API cannot be combined with in one request, as a refusal.
///
/// # Errors
///
/// A refusal when another way of adding fields to a hit is asked for.
pub fn check_alone(others: bool) -> Result<(), ShardError> {
    if others {
        return Err(unsupported(
            "[fields] together with [docvalue_fields], [script_fields] or [collapse]",
        ));
    }
    Ok(())
}

/// `FieldFetcher.create`: each entry's mapped fields over `mapping`, in the
/// order the hit's `fields` object writes them.
///
/// # Errors
///
/// OpenSearch's refusal of a `format` on a field that takes none, or this
/// node's for a field it does not read.
pub fn resolve(entries: &[DocValueField], mapping: &Mapping) -> Result<Vec<HitField>, ShardError> {
    // ALLOC: one field a distinct matched name, at most MAX_FIELDS.
    let mut out: Vec<HitField> = Vec::new();
    for entry in entries {
        let name = entry.field.as_str();
        // ALLOC: one name a matched field, as above.
        let mut names: Vec<&str> = if name.contains('*') {
            // ALLOC: one name a matched field, at most MAX_FIELDS.
            mapping.names().filter(|n| simple_match(name, n)).collect()
        } else {
            // ALLOC: one name.
            vec![name]
        };
        if name.contains('*') {
            // ALLOC: one index a name, as above.
            let mut order = Vec::new();
            java_hash_order(&names, &mut order);
            names = order
                .iter()
                .filter_map(|&i| names.get(i).copied())
                // ALLOC: the matched names once more, in order, as above.
                .collect();
        }
        for n in names {
            let Some(field) = mapping.get(n) else {
                continue;
            };
            let ty = source_type(n, field, mapping, entry.format.as_deref())?;
            if out.iter().all(|f| f.name != n) {
                out.push(HitField {
                    // ALLOC: one name a distinct field, as above.
                    name: n.to_owned(),
                    kind: ValueKind::Source,
                    times: 1,
                    script: None,
                    source: Some(SourceFetch {
                        // ALLOC: one path a distinct field, as above.
                        path: field.fetch.parent.as_deref().unwrap_or(n).to_owned(),
                        ty,
                    }),
                    ltr: None,
                });
            }
        }
    }
    // ALLOC: one name a resolved field, as above.
    let names: Vec<&str> = out.iter().map(|f| f.name.as_str()).collect();
    // ALLOC: one index a resolved field, as above.
    let mut order = Vec::new();
    java_hash_order(&names, &mut order);
    // ALLOC: the resolved fields once more, in order, as above.
    Ok(order.iter().filter_map(|&i| out.get(i).cloned()).collect())
}

/// How the mapped field `name` reads, or the refusal `valueFetcher` gives.
fn source_type(
    name: &str,
    field: &crate::mapping::Field,
    mapping: &Mapping,
    format: Option<&str>,
) -> Result<SourceType, ShardError> {
    // ALLOC: error path; one message.
    let refuse = |what: &str| unsupported(&format!("[fields] of the field [{name}] with {what},"));
    let no_format = |type_name: &str| {
        // ALLOC: error path; one message.
        bad_request(format!(
            "Field [{name}] of type [{type_name}] doesn't support formats."
        ))
    };
    let ty = match &field.ty {
        FieldType::Unsupported(what) => {
            // ALLOC: error path; one message.
            return Err(unsupported(&format!("[fields] of the {what} [{name}]")));
        }
        FieldType::Text if format.is_some() => {
            // ALLOC: error path; one message.
            return Err(bad_request(format!(
                "Field [{name}] doesn't support formats."
            )));
        }
        FieldType::Keyword if format.is_some() => return Err(no_format("keyword")),
        FieldType::Int { name: ty, .. } if format.is_some() => return Err(no_format(ty)),
        FieldType::Long if format.is_some() => return Err(no_format("long")),
        FieldType::Double if format.is_some() => return Err(no_format("double")),
        FieldType::Float if format.is_some() => return Err(no_format("float")),
        FieldType::HalfFloat if format.is_some() => return Err(no_format("half_float")),
        FieldType::Scaled(_) if format.is_some() => return Err(no_format("scaled_float")),
        FieldType::Date if format.is_some() => return Err(unsupported("a [fields] format")),
        FieldType::Text => SourceType::Text,
        FieldType::Keyword => SourceType::Keyword(field.fetch.ignore_above),
        FieldType::Int { min, max, .. } => SourceType::Int(*min, *max),
        FieldType::Long => SourceType::Long,
        FieldType::Double => SourceType::Double,
        FieldType::Float => SourceType::Float,
        FieldType::HalfFloat => SourceType::HalfFloat,
        FieldType::Scaled(bits) => SourceType::Scaled(*bits),
        FieldType::Date => SourceType::Date,
    };
    if let Some(option) = field.fetch.unserved {
        // ALLOC: error path; one message.
        return Err(refuse(&format!("[{option}]")));
    }
    if mapping.is_copy_target(name) {
        return Err(refuse("a [copy_to] into it"));
    }
    Ok(ty)
}

/// One value, read as its field's mapper reads it.
enum Out {
    Text(String),
    Number(i64),
    Double(f64),
    Float(f32),
    Date(i64),
}

/// `XContentMapValues.extractValue`'s result: a source value, or the results
/// of a list's elements.
enum Found<'a> {
    Value(&'a Value<'a>),
    List(Vec<Found<'a>>),
}

/// `XContentMapValues.extractValue(pathElements, index, current, null)`.
fn extract<'a>(path: &[&str], index: usize, current: &'a Value<'a>) -> Option<Found<'a>> {
    if let Value::Array(items) = current {
        // ALLOC: one result an element, bounded by the stored source.
        return Some(Found::List(
            items
                .iter()
                .filter_map(|o| extract(path, index, o))
                // ALLOC: one result an element, bounded by the stored source.
                .collect(),
        ));
    }
    if index == path.len() {
        return (!matches!(current, Value::Null)).then_some(Found::Value(current));
    }
    let Value::Object(entries) = current else {
        return None;
    };
    let get = |key: &str| entries.iter().find(|(k, _)| *k == key).map(|(_, v)| v);
    // ALLOC: one key a path element, bounded by the path.
    let mut key = path.get(index).copied().unwrap_or_default().to_owned();
    let mut next = index.saturating_add(1);
    let mut value = get(&key);
    while value.is_none_or(|v| matches!(v, Value::Null)) && next != path.len() {
        key.push('.');
        key.push_str(path.get(next).copied().unwrap_or_default());
        value = get(&key);
        next = next.saturating_add(1);
    }
    let value = value?;
    extract(path, next, value)
}

/// `Double.toString` for a source number, or the digits of an integer.
fn number_text(text: &str) -> String {
    if text.bytes().any(|b| matches!(b, b'.' | b'e' | b'E')) {
        return crate::template::java_double(text.parse::<f64>().unwrap_or(f64::NAN));
    }
    let digits = text.trim_start_matches('-');
    if digits.bytes().all(|b| b == b'0') {
        // ALLOC: one number's text, bounded by the stored source.
        return "0".to_owned();
    }
    // ALLOC: as above.
    text.to_owned()
}

/// `value.toString()` of a source value.
fn to_text(v: &Value<'_>) -> io::Result<String> {
    match v {
        // ALLOC: one value's text, bounded by the stored source.
        Value::Str(s) => Ok((*s).to_owned()),
        Value::Num(t) => Ok(number_text(t)),
        // ALLOC: one word.
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(io::Error::other("a [fields] value that is an object")),
    }
}

/// A whole number from a source value, truncating a decimal part as `coerce` does.
fn to_whole(v: &Value<'_>) -> io::Result<i64> {
    let bad = || io::Error::other("a [fields] value that is not a number");
    let text = match v {
        Value::Num(t) | Value::Str(t) => *t,
        _ => return Err(bad()),
    };
    let plain = text.strip_prefix(['-', '+']).unwrap_or(text);
    if plain.is_empty() || !plain.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        if matches!(v, Value::Num(_)) {
            // An exponent: a double, truncated.
            return float_whole(text);
        }
        return Err(bad());
    }
    let whole = plain.split('.').next().unwrap_or("");
    let n: i128 = whole.parse().map_err(|_| bad())?;
    let n = if text.starts_with('-') {
        n.saturating_neg()
    } else {
        n
    };
    i64::try_from(n).map_err(|_| io::Error::other("a [fields] value out of range"))
}

fn float_whole(text: &str) -> io::Result<i64> {
    let d: f64 = text
        .parse()
        .map_err(|_| io::Error::other("a [fields] value that is not a number"))?;
    // A `long` of Java is `-2^63..2^63`; `as` would saturate beyond it.
    if !d.is_finite() || !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&d) {
        return Err(io::Error::other("a [fields] value out of range"));
    }
    // ARITH: none; `as` truncates towards zero, as the Java cast does, and the range is checked.
    #[allow(clippy::cast_possible_truncation)]
    Ok(d as i64)
}

/// `Double.parseDouble` of a source number or string.
fn to_double(v: &Value<'_>) -> io::Result<f64> {
    let (Value::Num(t) | Value::Str(t)) = v else {
        return Err(io::Error::other("a [fields] value that is not a number"));
    };
    crate::numbers::java_double(t).map_err(|e| io::Error::other(e.reason))
}

/// `Float.floatValue` of a source number or string: the nearest float, finite.
fn to_float(v: &Value<'_>) -> io::Result<f32> {
    // Java's `floatValue` rounds to the nearest float, as `as` does.
    #[allow(clippy::cast_possible_truncation)]
    let f = to_double(v)? as f32;
    finite(f64::from(f), "float")?;
    Ok(f)
}

/// `HALF_FLOAT.parse`: a float that is finite as a half.
fn to_half(v: &Value<'_>) -> io::Result<f32> {
    // Java's `floatValue` rounds to the nearest float, as `as` does.
    #[allow(clippy::cast_possible_truncation)]
    let f = to_double(v)? as f32;
    let half = lucene_util::half_float::half_float_to_sortable_short(f);
    finite(
        f64::from(lucene_util::half_float::sortable_short_to_half_float(half)),
        "half_float",
    )?;
    Ok(f)
}

/// A value the mapper accepts: `NumberType.parse` refuses infinities and NaN.
fn finite(d: f64, ty: &str) -> io::Result<f64> {
    if d.is_finite() {
        Ok(d)
    } else {
        // ALLOC: error path; one message.
        Err(io::Error::other(format!(
            "[{ty}] supports only finite values in a [fields] value"
        )))
    }
}

/// `parseSourceValue` of one source value; none when the mapper skips it.
fn parse(ty: &SourceType, v: &Value<'_>) -> io::Result<Option<Out>> {
    Ok(match ty {
        SourceType::Text => Some(Out::Text(to_text(v)?)),
        SourceType::Keyword(above) => {
            let text = to_text(v)?;
            let units = i64::try_from(text.encode_utf16().count()).unwrap_or(i64::MAX);
            above.is_none_or(|a| units <= a).then_some(Out::Text(text))
        }
        SourceType::Int(min, max) => {
            if matches!(v, Value::Str("")) {
                return Ok(None);
            }
            let n = to_whole(v)?;
            if n < *min || n > *max {
                return Err(io::Error::other("a [fields] value out of range"));
            }
            Some(Out::Number(n))
        }
        SourceType::Long => {
            if matches!(v, Value::Str("")) {
                return Ok(None);
            }
            Some(Out::Number(to_whole(v)?))
        }
        SourceType::Double | SourceType::Float | SourceType::HalfFloat | SourceType::Scaled(_)
            if matches!(v, Value::Str("")) =>
        {
            return Ok(None);
        }
        SourceType::Double => Some(Out::Double(finite(to_double(v)?, "double")?)),
        SourceType::Float => Some(Out::Float(to_float(v)?)),
        SourceType::HalfFloat => Some(Out::Float(to_half(v)?)),
        SourceType::Scaled(bits) => {
            let factor = f64::from_bits(*bits);
            let d = finite(to_double(v)?, "scaled_float")?;
            // `Math.round(value * factor) / factor`.
            Some(Out::Double((d * factor + 0.5).floor() / factor))
        }
        SourceType::Date => {
            let millis = parse_to_millis(&to_text(v)?, false)
                .map_err(|_| io::Error::other("a [fields] value that is not a date"))?;
            Some(Out::Date(millis))
        }
    })
}

/// `SourceValueFetcher.fetchValues` of `fetch` over the source object.
fn fetch(fetch: &SourceFetch, source: &Value<'_>) -> io::Result<Vec<Out>> {
    // ALLOC: one element a path segment, bounded by the field's name.
    let path: Vec<&str> = fetch.path.split('.').collect();
    let Some(found) = extract(&path, 0, source) else {
        // ALLOC: empty.
        return Ok(Vec::new());
    };
    // ALLOC: one value a source value of the field, bounded by the stored source.
    let mut values = Vec::new();
    let mut queue = VecDeque::new();
    queue.push_back(found);
    while let Some(next) = queue.pop_front() {
        match next {
            Found::List(items) => queue.extend(items),
            Found::Value(v) => values.extend(parse(&fetch.ty, v)?),
        }
    }
    Ok(values)
}

/// Writes `,"fields":{...}` for a hit whose stored `_source` is `source`, or
/// nothing when none of `fields` has a value there.
///
/// # Errors
///
/// A write failure, or a `_source` that is not a JSON object or holds a value
/// its field cannot read.
pub fn write_fields(out: &mut dyn Write, fields: &[HitField], source: &[u8]) -> io::Result<()> {
    if !fields.iter().any(|f| f.source.is_some()) {
        return Ok(());
    }
    // ALLOC: the source's tokens and text, bounded by the stored source,
    // which the caller reserved.
    let mut rec = Recorder::new(scratch_bytes(source.len()));
    let mut tokenizer = crate::dsl::source::tokenizer();
    tokenizer
        .feed(source, &mut rec)
        .map_err(|e| io::Error::other(e.into_failure().reason))?;
    tokenizer
        .finish(&mut rec)
        .map_err(|e| io::Error::other(e.into_failure().reason))?;
    let mut p = rec.parser();
    let Some(Kind::StartObject) = p.next_token() else {
        return Err(io::Error::other("a _source that is not a JSON object"));
    };
    let root = read(&mut p, Kind::StartObject);
    let mut opened = false;
    for field in fields {
        let Some(spec) = &field.source else { continue };
        let values = fetch(spec, &root)?;
        if values.is_empty() {
            continue;
        }
        out.write_all(if opened { b"," } else { b",\"fields\":{" })?;
        opened = true;
        write_json_str(out, &field.name)?;
        out.write_all(b":[")?;
        for (n, value) in values.iter().enumerate() {
            if n > 0 {
                out.write_all(b",")?;
            }
            match value {
                Out::Text(t) => write_json_str(out, t)?,
                Out::Number(n) => write!(out, "{n}")?,
                Out::Double(d) => out.write_all(crate::template::java_double(*d).as_bytes())?,
                Out::Float(f) => crate::response::write_java_float(out, *f)?,
                Out::Date(ms) => {
                    out.write_all(b"\"")?;
                    write_date(out, *ms)?;
                    out.write_all(b"\"")?;
                }
            }
        }
        out.write_all(b"]")?;
    }
    if opened {
        out.write_all(b"}")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "source_fields_tests.rs"]
mod tests;
