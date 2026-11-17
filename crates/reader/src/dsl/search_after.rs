//! `search_after`: the sort values a page starts after.
//!
//! Ported from `SearchAfterBuilder.fromXContent`, `buildFieldDoc` and
//! `convertValueFromSortType`
//! (server/src/main/java/org/opensearch/search/searchafter/SearchAfterBuilder.java),
//! and the checks `SearchService.parseSource` makes before building it
//! (server/src/main/java/org/opensearch/search/SearchService.java), at
//! OpenSearch 3.10.0. Each value is read as Jackson reads it, then converted
//! to its sort key's type as Java converts it: a number by `intValue`,
//! `longValue` or `floatValue`, anything else by parsing its text. A null for
//! a numeric key fails as Lucene's comparator fails, with a 500. Date math
//! other than a refused `now` is not served yet.

use std::io::Write as _;

use lucene_search::top_field::{FieldDoc, SortField, SortType};

use crate::aggs::metric::double::write_java_double;
use crate::date::{DateError, parse_to_millis};
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::failure::Failure;
use crate::numbers::java_double;
use crate::search::ShardError;

/// One `search_after` value, as `fromXContent` keeps it.
#[derive(Clone, Debug, PartialEq)]
pub enum AfterValue {
    /// A number that fits an int.
    Int(i32),
    /// A whole number that fits a long but not an int.
    Long(i64),
    /// A number with a fraction or an exponent.
    Double(f64),
    /// A string, or a whole number past a long's range as its text.
    Text(String),
    /// A boolean.
    Bool(bool),
    /// `null`.
    Null,
}

/// `fromXContent`, the parser on the array's start: at least one value,
/// each a string, number, boolean or null.
pub fn parse_search_after(p: &mut Parser<'_>) -> Result<Vec<AfterValue>, DslError> {
    // ALLOC: one value a search_after entry, bounded by the body's recording.
    let mut values = Vec::new();
    loop {
        let value = match p.next_token() {
            Some(Kind::EndArray) | None => break,
            Some(Kind::Number) => number(p.text()),
            // ALLOC: one string a value, bounded by the body's recording.
            Some(Kind::String) => AfterValue::Text(p.text().to_owned()),
            Some(Kind::Bool) => AfterValue::Bool(p.text() == "true"),
            Some(Kind::Null) => AfterValue::Null,
            Some(k) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "Expected [VALUE_STRING] or [VALUE_NUMBER] or [VALUE_BOOLEAN] or \
                         [VALUE_NULL] but found [{}] inside search_after.",
                        k.name()
                    ),
                ));
            }
        };
        values.push(value);
    }
    if values.is_empty() {
        return Err(DslError::illegal(
            "Values must contains at least one value.",
        ));
    }
    Ok(values)
}

/// Jackson's number type for `text`: an int, a long, a big integer kept as
/// its text, or a double.
fn number(text: &str) -> AfterValue {
    if text.contains(['.', 'e', 'E']) {
        return AfterValue::Double(text.parse().unwrap_or(f64::NAN));
    }
    if let Ok(n) = text.parse::<i32>() {
        AfterValue::Int(n)
    } else if let Ok(n) = text.parse::<i64>() {
        AfterValue::Long(n)
    } else {
        // ALLOC: one string a value, bounded by the body's recording.
        AfterValue::Text(text.to_owned())
    }
}

/// What a sort key's search-after value converts to: its
/// `SortField.Type`, and for a date the date format's parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterKey<'a> {
    /// `_doc`: an int.
    Doc,
    /// `_score`: a float.
    Score,
    /// An integer field: an int.
    Int(&'a str),
    /// A long field: a long, its text parsed as `DocValueFormat.RAW` parses.
    Long(&'a str),
    /// A date field: a long, its text parsed as a date.
    Date(&'a str),
    /// A keyword field: the value's text.
    Term,
}

impl<'a> AfterKey<'a> {
    /// The key of `field`, a date when `is_date` says its field is one.
    pub fn of(field: &'a SortField, is_date: impl Fn(&str) -> bool) -> Self {
        let name = field.field.as_str();
        match field.ty {
            SortType::Score => AfterKey::Score,
            SortType::Doc => AfterKey::Doc,
            SortType::Int => AfterKey::Int(name),
            _ if is_date(name) => AfterKey::Date(name),
            SortType::String => AfterKey::Term,
            _ => AfterKey::Long(name),
        }
    }

    /// The field name an error quotes: `null` for `_doc` and `_score`.
    fn name(self) -> &'a str {
        match self {
            AfterKey::Int(n) | AfterKey::Long(n) | AfterKey::Date(n) => n,
            AfterKey::Doc | AfterKey::Score | AfterKey::Term => "null",
        }
    }

    /// The boxed type whose unboxing a null fails, and its method.
    fn unboxed(self) -> Option<(&'static str, &'static str)> {
        match self {
            AfterKey::Doc | AfterKey::Int(_) => Some(("Integer", "intValue")),
            AfterKey::Score => Some(("Float", "floatValue")),
            AfterKey::Long(_) | AfterKey::Date(_) => Some(("Long", "longValue")),
            AfterKey::Term => None,
        }
    }
}

fn bad_request(reason: impl Into<String>) -> ShardError {
    ShardError {
        status: 400,
        failure: Failure::plain("illegal_argument_exception", reason),
    }
}

/// `SearchService.parseSource`'s checks and `buildFieldDoc`: the page start
/// for `values` under the sort `keys` (`None` for a sort by score alone),
/// sorting after every document whose values equal it.
pub fn field_doc(
    from: usize,
    values: &[AfterValue],
    keys: Option<&[AfterKey<'_>]>,
) -> Result<FieldDoc, ShardError> {
    if from > 0 {
        return Err(ShardError {
            status: 500,
            failure: Failure::opensearch(
                "search_exception",
                "`from` parameter must be set to 0 when `search_after` is used.",
            ),
        });
    }
    let Some(keys) = keys.filter(|k| !k.is_empty()) else {
        return Err(bad_request("Sort must contain at least one field."));
    };
    if keys.len() != values.len() {
        // ALLOC: error path; one message.
        return Err(bad_request(format!(
            "search_after has {} value(s) but sort has {}.",
            values.len(),
            keys.len()
        )));
    }
    let has_terms = keys.contains(&AfterKey::Term);
    // ALLOC: one value a sort key, at most MAX_SORT_KEYS.
    let mut longs = Vec::with_capacity(keys.len());
    // ALLOC: one term a sort key when the sort has a keyword key.
    let mut terms = Vec::with_capacity(if has_terms { keys.len() } else { 0 });
    let mut null = None;
    for (&key, value) in keys.iter().zip(values) {
        let mut term = None;
        let long = match (key, value) {
            (AfterKey::Term, v) => {
                term = text_of(v);
                0
            }
            (k, AfterValue::Null) => {
                null = null.or(k.unboxed());
                0
            }
            (k, v) => convert(k, v)?,
        };
        longs.push(long);
        if has_terms {
            terms.push(term);
        }
    }
    if let Some((class, method)) = null {
        return Err(ShardError {
            status: 500,
            failure: Failure::plain(
                "null_pointer_exception",
                // ALLOC: error path; one message.
                format!("Cannot invoke \"java.lang.{class}.{method}()\" because \"value\" is null"),
            ),
        });
    }
    Ok(FieldDoc {
        doc: i32::MAX,
        values: longs,
        terms,
    })
}

/// `value.toString()` as a keyword's bytes; `None` for null.
fn text_of(value: &AfterValue) -> Option<Vec<u8>> {
    // ALLOC: one term a keyword key, bounded by the body's recording.
    let mut out = Vec::new();
    // Writing to a Vec cannot fail.
    let _ = match value {
        AfterValue::Null => return None,
        AfterValue::Text(s) => out.write_all(s.as_bytes()),
        AfterValue::Int(n) => write!(out, "{n}"),
        AfterValue::Long(n) => write!(out, "{n}"),
        AfterValue::Double(d) => write_java_double(&mut out, *d),
        AfterValue::Bool(b) => write!(out, "{b}"),
    };
    Some(out)
}

/// The text Java parses a non-number value from.
fn text(value: &AfterValue) -> &str {
    match value {
        AfterValue::Text(s) => s,
        AfterValue::Bool(true) => "true",
        _ => "false",
    }
}

/// `convertValueFromSortType` for a numeric key, encoded as
/// [`FieldDoc::values`] holds it: a score as its float's bits.
fn convert(key: AfterKey<'_>, value: &AfterValue) -> Result<i64, ShardError> {
    let failed = |reason: String| ShardError {
        status: 400,
        failure: Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "Failed to parse search_after value for field [{}].",
                key.name()
            ),
        )
        .caused_by(Failure::plain("number_format_exception", reason)),
    };
    // `Number.intValue`, `longValue` and `floatValue`, as Java narrows.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let number = match *value {
        AfterValue::Int(n) => Some((n, i64::from(n), n as f32)),
        AfterValue::Long(n) => Some((n as i32, n, n as f32)),
        AfterValue::Double(d) => Some((d as i32, d as i64, d as f32)),
        _ => None,
    };
    let text = text(value);
    match key {
        AfterKey::Doc | AfterKey::Int(_) => Ok(i64::from(match number {
            Some((int, _, _)) => int,
            None => text.parse::<i32>().map_err(|_| failed(input(text)))?,
        })),
        AfterKey::Score => {
            let score = match number {
                Some((_, _, float)) => float,
                None => java_float(text).map_err(failed)?,
            };
            Ok(i64::from(score.to_bits()))
        }
        AfterKey::Long(_) => match number {
            Some((_, long, _)) => Ok(long),
            None => raw_long(text).map_err(failed),
        },
        AfterKey::Date(_) => match number {
            Some((_, long, _)) => Ok(long),
            None => date(text),
        },
        // A keyword key never converts a number.
        AfterKey::Term => Ok(0),
    }
}

/// `NumberFormatException.forInputString`'s message.
fn input(text: &str) -> String {
    // ALLOC: error path; one message.
    format!("For input string: \"{text}\"")
}

/// `FloatingDecimal`'s message for a text that trims to nothing, else
/// `java_double`'s.
fn number_message(text: &str) -> String {
    if text.trim_matches(|c: char| c <= ' ').is_empty() {
        // ALLOC: error path; one message.
        return "empty String".to_owned();
    }
    input(text)
}

/// `Float.parseFloat`: `Double.parseDouble`'s syntax, read as a float.
fn java_float(text: &str) -> Result<f32, String> {
    if java_double(text).is_err() {
        return Err(number_message(text));
    }
    let t = text.trim_matches(|c: char| c <= ' ');
    let body = t.strip_suffix(['d', 'D', 'f', 'F']).unwrap_or(t);
    body.parse::<f32>().map_err(|_| number_message(text))
}

/// `DocValueFormat.RAW.parseLong`: a long, else a double rounded down.
fn raw_long(text: &str) -> Result<i64, String> {
    if let Ok(n) = text.parse::<i64>() {
        return Ok(n);
    }
    let d = java_double(text).map_err(|_| number_message(text))?;
    // `Math.round` of a whole double saturates, and a NaN is 0, as `as` does.
    #[allow(clippy::cast_possible_truncation)]
    let n = d.floor() as i64;
    Ok(n)
}

/// `DocValueFormat.DateTime.parseLong` with a clock that refuses `now`.
fn date(text: &str) -> Result<i64, ShardError> {
    parse_to_millis(text, false).map_err(|e| {
        let failure = if e == DateError::Math && text.starts_with("now") {
            Failure::opensearch("parse_exception", "could not read the current timestamp")
                .caused_by(Failure::plain(
                    "illegal_state_exception",
                    "now() is not allowed in [search_after] key",
                ))
        } else {
            e.into_failure(text)
        };
        ShardError {
            status: 400,
            failure,
        }
    })
}

#[cfg(test)]
#[path = "search_after_tests.rs"]
mod tests;
