//! Numbers as `NumberFieldMapper.NumberType.parse(Object, coerce)` reads
//! them, for numeric `null_value`s now and document values later.
//!
//! Differences from the Java: `long`'s fallback through `BigDecimal` for
//! text `Long.parseLong` refuses (`"1e3"`, `"12.0"`) goes through a double
//! here, so such text past 2^53 loses its low digits where the Java's keeps
//! them.

use super::error::MappingError;
use super::json::{Json, is_integral, java_double, java_float};
use lucene_util::half_float::{half_float_to_sortable_short, sortable_short_to_half_float};

/// A numeric field's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NumType {
    /// `byte`
    Byte,
    /// `short`
    Short,
    /// `integer`
    Integer,
    /// `long`
    Long,
    /// `float`
    Float,
    /// `double`
    Double,
    /// `half_float`
    HalfFloat,
    /// `scaled_float`
    ScaledFloat,
}

impl NumType {
    /// The type's name in a mapping.
    pub fn name(self) -> &'static str {
        match self {
            NumType::Byte => "byte",
            NumType::Short => "short",
            NumType::Integer => "integer",
            NumType::Long => "long",
            NumType::Float => "float",
            NumType::Double => "double",
            NumType::HalfFloat => "half_float",
            NumType::ScaledFloat => "scaled_float",
        }
    }
}

/// A parsed number of a [`NumType`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    /// A `byte`, `short` or `integer`.
    Int(i32),
    /// A `long`.
    Long(i64),
    /// A `float`.
    Float(f32),
    /// A `double`.
    Double(f64),
}

impl Num {
    /// The number as Java prints its boxed value.
    pub fn java_string(&self) -> String {
        match self {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            Num::Int(v) => v.to_string(),
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            Num::Long(v) => v.to_string(),
            Num::Float(v) => java_float(*v),
            Num::Double(v) => java_double(*v),
        }
    }
}

/// `Double.parseDouble`: optional surrounding whitespace, a sign, digits with
/// an optional fraction and exponent, an optional `d`/`f` suffix, or `NaN`
/// and `Infinity`. Hexadecimal floating point is not taken.
pub fn java_parse_double(text: &str) -> Option<f64> {
    let t = text.trim_matches(|c: char| c <= ' ');
    let unsigned = t.strip_prefix(['+', '-']).unwrap_or(t);
    let negative = t.starts_with('-');
    match unsigned {
        "NaN" => return Some(f64::NAN),
        "Infinity" => {
            return Some(if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            });
        }
        _ => {}
    }
    let body = unsigned
        .strip_suffix(['d', 'D', 'f', 'F'])
        .unwrap_or(unsigned);
    let b = body.as_bytes();
    let digits = |from: usize| {
        b.get(from..)
            .map_or(0, |r| r.iter().take_while(|c| c.is_ascii_digit()).count())
    };
    let whole = digits(0);
    let mut at = whole;
    let mut fraction = 0;
    if b.get(at) == Some(&b'.') {
        fraction = digits(at.saturating_add(1));
        at = at.saturating_add(1).saturating_add(fraction);
    }
    if whole.saturating_add(fraction) == 0 {
        return None;
    }
    if matches!(b.get(at), Some(b'e' | b'E')) {
        at = at.saturating_add(1);
        if matches!(b.get(at), Some(b'+' | b'-')) {
            at = at.saturating_add(1);
        }
        let exponent = digits(at);
        if exponent == 0 {
            return None;
        }
        at = at.saturating_add(exponent);
    }
    if at != b.len() {
        return None;
    }
    let v = body.parse::<f64>().ok()?;
    Some(if negative { -v } else { v })
}

/// What Jackson reads from a JSON value, as the Java's `value.toString()`.
fn as_text(node: &Json) -> String {
    node.java_string()
}

/// `objectToDouble`.
fn to_double(node: &Json) -> Result<f64, MappingError> {
    let text = as_text(node);
    java_parse_double(&text).ok_or_else(|| MappingError::number_format(&text))
}

/// `scaling_factor`: a finite positive double.
///
/// # Errors
///
/// A value that is no number, or no finite positive one.
pub fn scaling_factor(node: &Json) -> Result<f64, MappingError> {
    let v = to_double(node)?;
    if !v.is_finite() || v <= 0.0 {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::illegal_argument(&format!(
            "[scaling_factor] must be a positive number, got [{}]",
            java_double(v)
        )));
    }
    Ok(v)
}

/// Parses `node` as `type.parse(node, coerce)` does.
///
/// # Errors
///
/// `NumberFormatException` for text that is no number, and
/// `IllegalArgumentException` for a value out of the type's range, with a
/// decimal part when `coerce` is false, or not finite for a float type.
pub fn parse(ty: NumType, node: &Json, coerce: bool) -> Result<Num, MappingError> {
    let shown = as_text(node);
    match ty {
        NumType::Byte | NumType::Short | NumType::Integer => {
            let (min, max, what) = match ty {
                NumType::Byte => (f64::from(i8::MIN), f64::from(i8::MAX), "a byte"),
                NumType::Short => (f64::from(i16::MIN), f64::from(i16::MAX), "a short"),
                _ => (f64::from(i32::MIN), f64::from(i32::MAX), "an integer"),
            };
            let v = to_double(node)?;
            if v < min || v > max {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::illegal_argument(&format!(
                    "Value [{shown}] is out of range for {what}"
                )));
            }
            if !coerce && v % 1.0 != 0.0 {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::illegal_argument(&format!(
                    "Value [{shown}] has a decimal part"
                )));
            }
            // In range, so the truncation is the Java's `(int)` cast.
            #[allow(clippy::cast_possible_truncation)]
            Ok(Num::Int(v as i32))
        }
        NumType::Long => {
            if let Json::Num(t) = node
                && is_integral(t)
                && let Ok(v) = t.parse::<i64>()
            {
                return Ok(Num::Long(v));
            }
            let v = to_double(node)?;
            // `(double) Long.MAX_VALUE` is 2^63.
            if !(-9.223_372_036_854_776e18..=9.223_372_036_854_776e18).contains(&v) {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::illegal_argument(&format!(
                    "Value [{shown}] is out of range for a long"
                )));
            }
            if !coerce && v % 1.0 != 0.0 {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::illegal_argument(&format!(
                    "Value [{shown}] has a decimal part"
                )));
            }
            if let Ok(exact) = shown.parse::<i64>() {
                return Ok(Num::Long(exact));
            }
            if v >= 9.223_372_036_854_776e18 {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::illegal_argument(&format!(
                    "Value [{shown}] is out of range for a long"
                )));
            }
            // Range-checked above; the cast truncates toward zero as `toBigInteger` does.
            #[allow(clippy::cast_possible_truncation)]
            Ok(Num::Long(v as i64))
        }
        NumType::Float => {
            let v = to_double(node)?;
            #[allow(clippy::cast_possible_truncation)]
            let f = v as f32;
            if !f.is_finite() {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::illegal_argument(&format!(
                    "[float] supports only finite values, but got [{}]",
                    java_float(f)
                )));
            }
            Ok(Num::Float(f))
        }
        NumType::HalfFloat => {
            let v = to_double(node)?;
            #[allow(clippy::cast_possible_truncation)]
            let f = v as f32;
            let half = sortable_short_to_half_float(half_float_to_sortable_short(f));
            if !half.is_finite() {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::illegal_argument(&format!(
                    "[half_float] supports only finite values, but got [{}]",
                    java_float(f)
                )));
            }
            Ok(Num::Float(f))
        }
        NumType::ScaledFloat => Ok(Num::Double(to_double(node)?)),
        NumType::Double => {
            let v = to_double(node)?;
            if !v.is_finite() {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::illegal_argument(&format!(
                    "[double] supports only finite values, but got [{}]",
                    java_double(v)
                )));
            }
            Ok(Num::Double(v))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(t: &str) -> Json {
        Json::Num(t.into())
    }

    #[test]
    fn parses_like_number_type() {
        assert_eq!(
            parse(NumType::Long, &Json::Str("12".into()), false).unwrap(),
            Num::Long(12)
        );
        assert_eq!(
            parse(NumType::Integer, &num("7"), false).unwrap(),
            Num::Int(7)
        );
        assert_eq!(
            parse(NumType::Double, &num("1.5"), false).unwrap(),
            Num::Double(1.5)
        );
        assert_eq!(
            parse(NumType::Float, &num("1.5"), false).unwrap(),
            Num::Float(1.5)
        );
        assert_eq!(
            parse(NumType::Long, &num("9223372036854775807"), false).unwrap(),
            Num::Long(i64::MAX)
        );
        let e = parse(NumType::Long, &Json::Str("abc".into()), false).unwrap_err();
        assert_eq!(
            (e.ty, e.reason.as_str()),
            ("number_format_exception", "For input string: \"abc\"")
        );
        let e = parse(NumType::Byte, &num("300"), false).unwrap_err();
        assert_eq!(e.reason, "Value [300] is out of range for a byte");
        let e = parse(NumType::Integer, &num("1.5"), false).unwrap_err();
        assert_eq!(e.reason, "Value [1.5] has a decimal part");
        assert_eq!(
            parse(NumType::Integer, &num("1.5"), true).unwrap(),
            Num::Int(1)
        );
        assert!(parse(NumType::Double, &Json::Str("NaN".into()), false).is_err());
        assert_eq!(java_parse_double(" 2.5d "), Some(2.5));
        assert_eq!(java_parse_double("x"), None);
        assert_eq!(java_parse_double("-Infinity"), Some(f64::NEG_INFINITY));
    }

    #[test]
    fn integral_types_take_their_range_edges() {
        for (ty, min, max) in [
            (NumType::Byte, "-128", "127"),
            (NumType::Short, "-32768", "32767"),
            (NumType::Integer, "-2147483648", "2147483647"),
        ] {
            assert!(parse(ty, &num(min), false).is_ok(), "{min}");
            assert!(parse(ty, &num(max), false).is_ok(), "{max}");
        }
        for (ty, out, what) in [
            (NumType::Short, "32768", "a short"),
            (NumType::Short, "-32769", "a short"),
            (NumType::Byte, "-129", "a byte"),
        ] {
            let e = parse(ty, &num(out), false).unwrap_err();
            assert_eq!(
                e.reason,
                format!("Value [{out}] is out of range for {what}")
            );
        }
    }

    #[test]
    fn parses_doubles_as_double_parse_double() {
        // Each case as `Double.parseDouble` answered it on a JVM.
        for nan in ["NaN", "-NaN", "+NaN", " NaN "] {
            assert!(java_parse_double(nan).is_some_and(f64::is_nan), "{nan}");
        }
        for bad in [
            "NaNd",
            "Infinityd",
            "..1",
            "-",
            "+",
            "",
            ".",
            "1e",
            "1e+",
            "1.5x",
            "e5",
            "1d5",
            "inf",
            "nan",
            "1e5dd",
            "1.2.3",
            "--1",
            "0x10",
        ] {
            assert_eq!(java_parse_double(bad), None, "{bad}");
        }
        for (text, v) in [
            ("Infinity", f64::INFINITY),
            ("+Infinity", f64::INFINITY),
            (" 1.5d ", 1.5),
            ("1e5f", 100_000.0),
            ("1.", 1.0),
            (".5", 0.5),
            ("-2.5E-1", -0.25),
            ("+3", 3.0),
            ("1e+2D", 100.0),
            ("7F", 7.0),
        ] {
            assert_eq!(java_parse_double(text), Some(v), "{text}");
        }
    }
}
