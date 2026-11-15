//! Query values for integer and long fields, read as `NumberFieldMapper`
//! reads them.
//!
//! Ported from `NumberFieldMapper.NumberType.INTEGER` and `LONG`'s `parse`,
//! `termQuery` and `rangeQuery`, `#objectToDouble`, `#hasDecimalPart` and
//! `#signum`
//! (server/src/main/java/org/opensearch/index/mapper/NumberFieldMapper.java),
//! and `Numbers.toLong` (libs/common/src/main/java/org/opensearch/common/Numbers.java),
//! at OpenSearch 3.10.0. A value is a JSON number or string, parsed as Java's
//! `Double.parseDouble` would; `byte` and `short` fields query as integers,
//! as their types delegate to `INTEGER`. A long beyond 2^53 with a decimal
//! part loses its precision, where Java's `BigDecimal` keeps it.

use crate::dsl::Value;
use crate::failure::Failure;

/// `Double.parseDouble`: surrounding whitespace and a trailing type suffix
/// are allowed; anything else that is not a decimal number fails as Java
/// fails.
pub fn java_double(text: &str) -> Result<f64, Failure> {
    let t = text.trim_matches(|c: char| c <= ' ');
    let body = t.strip_suffix(['d', 'D', 'f', 'F']).unwrap_or(t);
    let unsigned = body.strip_prefix(['+', '-']).unwrap_or(body);
    let named = unsigned == "Infinity" || unsigned == "NaN";
    let decimal = !unsigned.is_empty()
        && unsigned
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
        && unsigned.bytes().any(|b| b.is_ascii_digit());
    if !named && !decimal {
        return Err(number_format(text));
    }
    body.parse::<f64>().map_err(|_| number_format(text))
}

fn number_format(text: &str) -> Failure {
    // ALLOC: error path; one message.
    Failure::plain(
        "number_format_exception",
        format!("For input string: \"{text}\""),
    )
}

fn out_of_range(value: &Value, type_name: &str) -> Failure {
    Failure::plain(
        "illegal_argument_exception",
        // ALLOC: error path; one message.
        format!("Value [{}] is out of range for {type_name}", value.text()),
    )
}

/// `hasDecimalPart`: a number or a string with a fractional part. A boolean
/// has none, and fails later when it is parsed.
pub fn has_decimal_part(value: &Value) -> Result<bool, Failure> {
    match value {
        Value::Bool(_) => Ok(false),
        Value::Str(s) | Value::Number(s) => Ok(java_double(s)? % 1.0 != 0.0),
    }
}

/// `signum`: the sign of the value's double, `Math.signum`.
fn signum(value: &Value) -> Result<f64, Failure> {
    let d = java_double(value.text())?;
    Ok(if d > 0.0 {
        1.0
    } else if d < 0.0 {
        -1.0
    } else {
        d
    })
}

/// `INTEGER.parse(value, true)`: within an int's range, truncated toward
/// zero.
pub fn parse_int(value: &Value) -> Result<i32, Failure> {
    let d = java_double(value.text())?;
    if d < f64::from(i32::MIN) || d > f64::from(i32::MAX) {
        return Err(out_of_range(value, "an integer"));
    }
    // A NaN is 0 and a value in range truncates, as Java's `(int)` cast does.
    #[allow(clippy::cast_possible_truncation)]
    let n = d as i32;
    Ok(n)
}

/// `LONG.parse(value, true)`: `objectToLong`, then `Numbers.toLong`.
pub fn parse_long(value: &Value) -> Result<i64, Failure> {
    let text = value.text();
    if let (Value::Number(_), Ok(n)) = (value, text.parse::<i64>()) {
        return Ok(n);
    }
    let d = java_double(text)?;
    // The bounds are the doubles Java compares against.
    #[allow(clippy::cast_precision_loss)]
    let (min, max) = (i64::MIN as f64, i64::MAX as f64);
    if d < min || d > max {
        return Err(out_of_range(value, "a long"));
    }
    if let Ok(n) = text.trim().parse::<i64>() {
        return Ok(n);
    }
    // `BigDecimal.toBigInteger` truncates toward zero; 2^63 itself compares
    // equal to `Long.MAX_VALUE` as a double but is out of range.
    if d.trunc() >= max {
        return Err(out_of_range(value, "a long"));
    }
    #[allow(clippy::cast_possible_truncation)]
    let n = d.trunc() as i64;
    Ok(n)
}

/// An inclusive range of a numeric field's values, or none when nothing
/// can match.
pub type Bounds<T> = Option<(T, T)>;

/// `INTEGER.rangeQuery`'s bounds: a decimal lower bound rounds up when
/// positive, a decimal upper bound down when negative, and an excluded
/// whole bound moves one step in.
pub fn int_range(
    from: Option<&Value>,
    to: Option<&Value>,
    include_lower: bool,
    include_upper: bool,
) -> Result<Bounds<i32>, Failure> {
    let mut l = i32::MIN;
    let mut u = i32::MAX;
    if let Some(v) = from {
        l = parse_int(v)?;
        let decimal = has_decimal_part(v)?;
        if (!decimal && !include_lower) || (decimal && signum(v)? > 0.0) {
            let Some(next) = l.checked_add(1) else {
                return Ok(None);
            };
            l = next;
        }
    }
    if let Some(v) = to {
        u = parse_int(v)?;
        let decimal = has_decimal_part(v)?;
        if (!decimal && !include_upper) || (decimal && signum(v)? < 0.0) {
            let Some(prev) = u.checked_sub(1) else {
                return Ok(None);
            };
            u = prev;
        }
    }
    Ok((l <= u).then_some((l, u)))
}

/// `LONG.rangeQuery`'s bounds, as [`int_range`]'s.
pub fn long_range(
    from: Option<&Value>,
    to: Option<&Value>,
    include_lower: bool,
    include_upper: bool,
) -> Result<Bounds<i64>, Failure> {
    let mut l = i64::MIN;
    let mut u = i64::MAX;
    if let Some(v) = from {
        l = parse_long(v)?;
        let decimal = has_decimal_part(v)?;
        if (!decimal && !include_lower) || (decimal && signum(v)? > 0.0) {
            let Some(next) = l.checked_add(1) else {
                return Ok(None);
            };
            l = next;
        }
    }
    if let Some(v) = to {
        u = parse_long(v)?;
        let decimal = has_decimal_part(v)?;
        if (!decimal && !include_upper) || (decimal && signum(v)? < 0.0) {
            let Some(prev) = u.checked_sub(1) else {
                return Ok(None);
            };
            u = prev;
        }
    }
    Ok((l <= u).then_some((l, u)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(s: &str) -> Value {
        Value::Number(s.into())
    }

    fn st(s: &str) -> Value {
        Value::Str(s.into())
    }

    fn reason(r: Result<impl std::fmt::Debug, Failure>) -> String {
        match r {
            Err(f) => f.reason,
            Ok(v) => panic!("parsed {v:?}"),
        }
    }

    #[test]
    fn doubles_parse_as_java_parses_them() {
        assert_eq!(java_double(" 12 ").ok(), Some(12.0));
        assert_eq!(java_double("1.5d").ok(), Some(1.5));
        assert_eq!(java_double("-1e2").ok(), Some(-100.0));
        assert_eq!(java_double("Infinity").ok(), Some(f64::INFINITY));
        assert!(java_double("NaN").is_ok_and(f64::is_nan));
        for bad in ["abc", "", "inf", ".", "1x", "--1", "true"] {
            assert_eq!(
                reason(java_double(bad)),
                format!("For input string: \"{bad}\"")
            );
        }
    }

    #[test]
    fn integers_are_bounded_and_truncated() {
        assert_eq!(parse_int(&num("2002")).ok(), Some(2002));
        assert_eq!(parse_int(&st("2010")).ok(), Some(2010));
        assert_eq!(parse_int(&num("-1.9")).ok(), Some(-1));
        assert_eq!(parse_int(&num("2147483647")).ok(), Some(i32::MAX));
        assert_eq!(
            reason(parse_int(&num("3000000000"))),
            "Value [3000000000] is out of range for an integer"
        );
        assert_eq!(
            reason(parse_int(&num("-2147483649"))),
            "Value [-2147483649] is out of range for an integer"
        );
        assert_eq!(
            reason(parse_int(&Value::Bool(true))),
            "For input string: \"true\""
        );
        assert_eq!(has_decimal_part(&num("2002.5")).ok(), Some(true));
        assert_eq!(has_decimal_part(&st("2002.0")).ok(), Some(false));
        assert_eq!(has_decimal_part(&Value::Bool(false)).ok(), Some(false));
    }

    #[test]
    fn longs_keep_their_precision() {
        assert_eq!(parse_long(&num("9223372036854775807")).ok(), Some(i64::MAX));
        assert_eq!(parse_long(&st("9223372036854775807")).ok(), Some(i64::MAX));
        assert_eq!(parse_long(&st(" 12 ")).ok(), Some(12));
        assert_eq!(parse_long(&num("-3.7")).ok(), Some(-3));
        assert_eq!(parse_long(&num("1e3")).ok(), Some(1000));
        assert_eq!(
            reason(parse_long(&num("9223372036854775808"))),
            "Value [9223372036854775808] is out of range for a long"
        );
        assert_eq!(
            reason(parse_long(&num("1e30"))),
            "Value [1e30] is out of range for a long"
        );
        assert_eq!(reason(parse_long(&st("x"))), "For input string: \"x\"");
    }

    #[test]
    fn range_bounds_round_as_opensearch_does() {
        let r = |f: Option<&str>, t: Option<&str>, il, iu| {
            int_range(f.map(num).as_ref(), t.map(num).as_ref(), il, iu)
                .ok()
                .flatten()
        };
        assert_eq!(
            r(Some("2001"), Some("2002"), true, true),
            Some((2001, 2002))
        );
        assert_eq!(
            r(Some("1999"), Some("2010"), false, false),
            Some((2000, 2009))
        );
        assert_eq!(r(Some("1.5"), None, true, true), Some((2, i32::MAX)));
        assert_eq!(r(Some("1.5"), None, false, true), Some((2, i32::MAX)));
        assert_eq!(r(Some("-1.5"), None, true, true), Some((-1, i32::MAX)));
        assert_eq!(r(None, Some("-1.5"), true, true), Some((i32::MIN, -2)));
        assert_eq!(r(None, Some("1.5"), true, false), Some((i32::MIN, 1)));
        assert_eq!(r(Some("2147483647"), None, false, true), None);
        assert_eq!(r(None, Some("-2147483648"), true, false), None);
        assert_eq!(r(Some("5"), Some("4"), true, true), None);
        assert_eq!(r(None, None, true, true), Some((i32::MIN, i32::MAX)));
        let l = |f: Option<&str>, t: Option<&str>, il, iu| {
            long_range(f.map(num).as_ref(), t.map(num).as_ref(), il, iu)
                .ok()
                .flatten()
        };
        assert_eq!(l(Some("1"), Some("3"), false, false), Some((2, 2)));
        assert_eq!(l(Some("1.5"), Some("-0.5"), true, true), None);
        assert_eq!(l(Some("0.5"), Some("2.5"), true, true), Some((1, 2)));
        assert_eq!(l(None, Some("-0.5"), true, true), Some((i64::MIN, -1)));
        assert_eq!(l(Some("9223372036854775807"), None, false, true), None);
        assert_eq!(l(None, Some("-9223372036854775808"), true, false), None);
        assert_eq!(l(None, None, true, true), Some((i64::MIN, i64::MAX)));
        assert!(int_range(Some(&st("x")), None, true, true).is_err());
        assert!(long_range(None, Some(&st("x")), true, true).is_err());
    }
}
