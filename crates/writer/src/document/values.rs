//! One mapped field's value from the current token, as each field mapper's
//! `parseCreateField` reads it: Jackson's number accessors and their
//! coercion rules, `Booleans.parseBoolean`, `textOrNull` and the date
//! formatters, with the Java's messages.

use crate::mapping::java_parse_double;
use crate::mapping::{FieldType, Formats, Leaf, MappingError, Num, NumType, Value};

use super::reader::{At, Tok};

/// One value a field indexes.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    /// A `text` field's text, to be analyzed.
    Text(String),
    /// A `keyword` field's term.
    Keyword(String),
    /// A number, of the field's type.
    Number(Num),
    /// A `date`, in milliseconds since the epoch.
    Date(i64),
    /// A `boolean`.
    Bool(bool),
}

/// 2^63, `(double) Long.MAX_VALUE`.
const LONG_BOUND: f64 = 9_223_372_036_854_775_808.0;

/// What a field takes from one value.
#[derive(Clone, Debug, PartialEq)]
pub enum Taken {
    /// A value to index.
    Value(FieldValue),
    /// Nothing: a `null` with no `null_value`, or a keyword over its
    /// `ignore_above`, which `KeywordFieldMapper` drops without adding the
    /// field to `_ignored`.
    Nothing,
}

/// `Location` as Jackson's coercion messages end.
fn jackson_at(at: At) -> String {
    // ALLOC: error path; one message.
    format!(
        "\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); byte offset: #{}]",
        at.end
    )
}

/// `InputCoercionException`, as OpenSearch reports it: twice, the second
/// as the first's cause.
fn coercion(message: &str, at: At) -> MappingError {
    // ALLOC: error path; one message.
    let reason = format!("{message}{}", jackson_at(at));
    MappingError::input_coercion(&reason).caused_by(MappingError::input_coercion(&reason))
}

/// `textOrNull`: the text of a string, number or flag.
pub fn text_or_null(tok: &Tok, at: At) -> Result<Option<String>, MappingError> {
    match tok {
        Tok::Null => Ok(None),
        Tok::Str(s) | Tok::Num(s) => Ok(Some(s.clone())),
        // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
        Tok::Bool(b) => Ok(Some(if *b { "true" } else { "false" }.to_owned())),
        // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
        other => Err(MappingError::illegal_state(&format!(
            "Can't get text on a {} at {}:{}",
            other.jackson_name(),
            at.line,
            at.column
        ))),
    }
}

/// Reads `tok` into `leaf`.
///
/// # Errors
///
/// The exception the Java field mapper throws, unwrapped: the caller adds
/// `failed to parse field [...]`.
pub fn take(leaf: &Leaf, tok: &Tok, at: At) -> Result<Taken, MappingError> {
    match leaf.ty() {
        FieldType::Text => Ok(match text_or_null(tok, at)? {
            Some(t) => Taken::Value(FieldValue::Text(t)),
            None => Taken::Nothing,
        }),
        FieldType::Keyword => keyword(leaf, tok, at),
        FieldType::Boolean => boolean(leaf, tok, at),
        FieldType::Date => date(leaf, tok, at),
        FieldType::Number(ty) => number(leaf, ty, tok, at),
    }
}

fn keyword(leaf: &Leaf, tok: &Tok, at: At) -> Result<Taken, MappingError> {
    let value = match text_or_null(tok, at)? {
        Some(t) => t,
        None => match leaf.get("null_value").and_then(Value::text) {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            Some(n) => n.to_owned(),
            None => return Ok(Taken::Nothing),
        },
    };
    let ignore_above = match leaf.get("ignore_above") {
        Some(Value::Int(n)) => usize::try_from(*n).unwrap_or(0),
        _ => usize::MAX,
    };
    // `String.length()` counts UTF-16 units.
    if value.encode_utf16().count() > ignore_above {
        return Ok(Taken::Nothing);
    }
    Ok(Taken::Value(FieldValue::Keyword(value)))
}

fn boolean(leaf: &Leaf, tok: &Tok, at: At) -> Result<Taken, MappingError> {
    let v = match tok {
        Tok::Null => match leaf.get("null_value") {
            Some(Value::Bool(b)) => *b,
            _ => return Ok(Taken::Nothing),
        },
        Tok::Bool(b) => *b,
        // `Booleans.parseBoolean(chars, offset, 0, false)` is the default.
        Tok::Str(s) if s.is_empty() => false,
        Tok::Str(s) if s == "true" => true,
        Tok::Str(s) if s == "false" => false,
        Tok::Str(s) => {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            return Err(MappingError::illegal_argument(&format!(
                "Failed to parse value [{s}] as only [true] or [false] are allowed."
            )));
        }
        other => {
            return Err(coercion(
                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                &format!(
                    "Current token ({}) not of boolean type",
                    other.jackson_name()
                ),
                at,
            ));
        }
    };
    Ok(Taken::Value(FieldValue::Bool(v)))
}

/// A date field's formats (validated when the mapping was parsed).
fn formats(leaf: &Leaf) -> (String, Option<Formats>) {
    let pattern = leaf
        .get("format")
        .and_then(Value::text)
        .unwrap_or(crate::mapping::DEFAULT_DATE_FORMAT)
        // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
        .to_owned();
    let parsed = Formats::parse(&pattern);
    (pattern, parsed)
}

/// Milliseconds for `text` in a field's `format`, or the Java's error.
pub fn parse_date(
    pattern: &str,
    formats: Option<&Formats>,
    text: &str,
) -> Result<i64, MappingError> {
    if let Some(f) = formats {
        if let Some(invalid) = f.invalid_day(text) {
            return Err(MappingError::date_time(&invalid));
        }
        if let Some(ms) = f.parse_millis(text) {
            return Ok(ms);
        }
    }
    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
    Err(MappingError::illegal_argument(&format!(
        "failed to parse date field [{text}] with format [{pattern}]"
    ))
    .caused_by(MappingError::date_time_parse(
        "Failed to parse with all enclosed parsers",
    )))
}

fn date(leaf: &Leaf, tok: &Tok, at: At) -> Result<Taken, MappingError> {
    let (pattern, parsed) = formats(leaf);
    let text = match text_or_null(tok, at)? {
        Some(t) => t,
        None => match leaf.get("null_value").and_then(Value::text) {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            Some(n) => n.to_owned(),
            None => return Ok(Taken::Nothing),
        },
    };
    let ms = parse_date(&pattern, parsed.as_ref(), &text)?;
    Ok(Taken::Value(FieldValue::Date(ms)))
}

/// The class name Java's messages use for a type's accessor.
fn class(ty: NumType) -> &'static str {
    match ty {
        NumType::Byte | NumType::Integer => "Integer",
        NumType::Short => "Short",
        NumType::Long => "Long",
        NumType::Float => "Float",
        NumType::Double => "Double",
    }
}

fn number(leaf: &Leaf, ty: NumType, tok: &Tok, at: At) -> Result<Taken, MappingError> {
    let coerce = leaf.get("coerce").is_none_or(Value::flag);
    let null_value = || match leaf.get("null_value") {
        Some(Value::Number(n)) => Taken::Value(FieldValue::Number(*n)),
        _ => Taken::Nothing,
    };
    let n = match tok {
        Tok::Null => return Ok(null_value()),
        Tok::Str(s) if coerce && s.is_empty() => return Ok(null_value()),
        Tok::Str(s) => {
            if !coerce {
                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                return Err(MappingError::illegal_argument(&format!(
                    "{} value passed as String",
                    class(ty)
                )));
            }
            from_string(ty, s)?
        }
        Tok::Num(t) => from_number(ty, t, coerce, at)?,
        other => {
            return Err(coercion(
                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                &format!(
                    "Current token ({}) not numeric, cannot use numeric value accessors",
                    other.jackson_name()
                ),
                at,
            ));
        }
    };
    Ok(Taken::Value(FieldValue::Number(n)))
}

fn finite(ty: NumType, n: Num) -> Result<Num, MappingError> {
    let (ok, shown) = match n {
        Num::Float(f) => (f.is_finite(), n.java_string()),
        Num::Double(d) => (d.is_finite(), n.java_string()),
        _ => return Ok(n),
    };
    if ok {
        Ok(n)
    } else {
        // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
        Err(MappingError::illegal_argument(&format!(
            "[{}] supports only finite values, but got [{shown}]",
            ty.name()
        )))
    }
}

/// A string value, coerced (`coerce` is true here).
fn from_string(ty: NumType, s: &str) -> Result<Num, MappingError> {
    let nfe = || MappingError::number_format(s);
    match ty {
        NumType::Long => to_long(s).map(Num::Long),
        NumType::Float => {
            let d = java_parse_double(s).ok_or_else(nfe)?;
            #[allow(clippy::cast_possible_truncation)]
            finite(ty, Num::Float(d as f32))
        }
        NumType::Double => finite(ty, Num::Double(java_parse_double(s).ok_or_else(nfe)?)),
        NumType::Integer | NumType::Short | NumType::Byte => {
            let d = java_parse_double(s).ok_or_else(nfe)?;
            let (min, max, what) = if ty == NumType::Short {
                (f64::from(i16::MIN), f64::from(i16::MAX), "a short")
            } else {
                (f64::from(i32::MIN), f64::from(i32::MAX), "an integer")
            };
            // NaN passes the Java's range check and its `(int)` cast reads it as 0.
            if d < min || d > max {
                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                return Err(MappingError::illegal_argument(&format!(
                    "Value [{s}] is out of range for {what}"
                )));
            }
            // In range: the Java's `(int)` cast truncates toward zero.
            #[allow(clippy::cast_possible_truncation)]
            let v = d as i32;
            byte_range(ty, v)
        }
    }
}

/// `ByteFieldMapper`'s check on the `int` it read.
fn byte_range(ty: NumType, v: i32) -> Result<Num, MappingError> {
    if ty == NumType::Byte && i8::try_from(v).is_err() {
        // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
        return Err(MappingError::illegal_argument(&format!(
            "Value [{v}] is out of range for a byte"
        )));
    }
    Ok(Num::Int(v))
}

/// `Numbers.toLong(text, true)`.
fn to_long(s: &str) -> Result<i64, MappingError> {
    // `Long.parseLong`: one optional sign, then digits, in range.
    if let Ok(v) = s.parse::<i64>() {
        return Ok(v);
    }
    let Some(d) = big_decimal(s) else {
        // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
        return Err(MappingError::illegal_argument(&format!(
            "For input string: \"{s}\""
        )));
    };
    // `(double) Long.MAX_VALUE` is 2^63; the Java compares exactly, which
    // differs only within a unit of the bounds.
    if !(-LONG_BOUND..LONG_BOUND).contains(&d) {
        // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
        return Err(MappingError::illegal_argument(&format!(
            "Value [{s}] is out of range for a long"
        )));
    }
    // In range: `toBigInteger` truncates toward zero, as the cast does.
    #[allow(clippy::cast_possible_truncation)]
    Ok(d.trunc() as i64)
}

/// `new BigDecimal(text)`: `[+-](digits[.digits]|.digits)[e[+-]digits]`.
/// Rust's float parse reads the same forms, and also `inf` and `nan`,
/// which have letters BigDecimal's form does not.
fn big_decimal(s: &str) -> Option<f64> {
    if !s
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return None;
    }
    s.parse::<f64>().ok()
}

/// A number token, through Jackson's accessor for the type.
fn from_number(ty: NumType, t: &str, coerce: bool, at: At) -> Result<Num, MappingError> {
    let full: f64 = t.parse().unwrap_or(f64::NAN);
    let integral = crate::mapping::json::is_integral(t);
    let range = |name: &str, min: &str, max: &str| {
        coercion(
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            &format!("Numeric value ({t}) out of range of `{name}` ({min} - {max})"),
            at,
        )
    };
    let lossy = |result: f64| {
        if !coerce && result != full {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            Err(MappingError::illegal_argument(&format!(
                "{} cannot be converted to {} without data loss",
                crate::mapping::json::java_double(full),
                class(ty)
            )))
        } else {
            Ok(())
        }
    };
    match ty {
        NumType::Float =>
        {
            #[allow(clippy::cast_possible_truncation)]
            finite(ty, Num::Float(full as f32))
        }
        NumType::Double => finite(ty, Num::Double(full)),
        NumType::Long => {
            let v = if integral {
                t.parse::<i64>()
                    .map_err(|_| range("long", "-9223372036854775808", "9223372036854775807"))?
            } else {
                if !(-LONG_BOUND..LONG_BOUND).contains(&full) {
                    return Err(range("long", "-9223372036854775808", "9223372036854775807"));
                }
                #[allow(clippy::cast_possible_truncation)]
                let v = full as i64;
                v
            };
            #[allow(clippy::cast_precision_loss)]
            lossy(v as f64)?;
            Ok(Num::Long(v))
        }
        NumType::Integer | NumType::Short | NumType::Byte => {
            let int_range = || range("int", "-2147483648", "2147483647");
            let v: i32 = if integral {
                t.parse::<i64>()
                    .ok()
                    .and_then(|v| i32::try_from(v).ok())
                    .ok_or_else(int_range)?
            } else {
                if !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&full) {
                    return Err(int_range());
                }
                #[allow(clippy::cast_possible_truncation)]
                let v = full as i32;
                v
            };
            if ty == NumType::Short && i16::try_from(v).is_err() {
                return Err(range("short", "-32768", "32767"));
            }
            lossy(f64::from(v))?;
            byte_range(ty, v)
        }
    }
}
