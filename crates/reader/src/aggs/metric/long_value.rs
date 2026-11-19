//! A JSON number or string read as a long, as `parser.longValue()` does.

use crate::dsl::error::DslError;
use crate::dsl::tokens::Kind;
use crate::failure::Failure;

/// `parser.longValue()` with coercion, the parser on a number or a string:
/// a number as Jackson reads it (a fraction truncated), a string as
/// `Numbers.toLong(text, true)` reads it.
pub(crate) fn long_value(kind: Kind, text: &str) -> Result<i64, DslError> {
    if let Ok(n) = text.parse::<i64>() {
        return Ok(n);
    }
    // 2^63, exact as a double.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    let decimal = is_big_decimal(text);
    let value = if decimal {
        text.parse::<f64>().ok()
    } else {
        None
    };
    if kind == Kind::Number {
        return match value {
            // ARITH: none; `as` truncates, and the value is inside a long.
            #[allow(clippy::cast_possible_truncation)]
            Some(v) if text.contains(['.', 'e', 'E']) && (-LIMIT..=LIMIT).contains(&v) => {
                Ok(v as i64)
            }
            _ => Err(DslError::Failure(Failure::plain(
                "input_coercion_exception",
                // ALLOC: error path; one message.
                format!(
                    "Numeric value ({text}) out of range of `long` (-9223372036854775808 - \
                     9223372036854775807)"
                ),
            ))),
        };
    }
    match value {
        // ARITH: none; `as` truncates toward zero as `toBigInteger` does,
        // and the value is inside a long.
        #[allow(clippy::cast_possible_truncation)]
        Some(v) if v > -LIMIT - 1.0 && v < LIMIT => Ok(v as i64),
        Some(_) => Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("Value [{text}] is out of range for a long"),
        )),
        // ALLOC: error path; one message.
        None => Err(DslError::illegal(format!("For input string: \"{text}\""))),
    }
}

/// Whether `new BigDecimal(text)` reads `text`: a sign, digits with at most
/// one point and at least one digit, and an exponent.
fn is_big_decimal(text: &str) -> bool {
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => (
            body.get(..at).unwrap_or_default(),
            body.get(at.saturating_add(1)..),
        ),
        None => (body, None),
    };
    let mut parts = mantissa.splitn(2, '.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next().unwrap_or_default();
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok =
        digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty());
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    mantissa_ok && exponent_ok
}
