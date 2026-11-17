//! `Double.toString`, as Jackson writes a `double` into a response.
//!
//! Java 19 and later (OpenSearch 3.10.0 runs on 21) print the shortest
//! decimal that reads back as the double, the one closest to it among
//! those, considering two digits when one would do; plain notation from
//! 10^-3 up to 10^7, always with a fraction, and computerized scientific
//! notation (`1.0E7`) outside it.

use std::io::{self, Write};

/// A double's significant digits, without a point, and the decimal
/// exponent of the first: `digits[0].digits[1..] * 10^exponent`.
struct Decimal {
    digits: [u8; 24],
    len: usize,
    exponent: i32,
}

impl Decimal {
    fn digits(&self) -> &[u8] {
        self.digits.get(..self.len).unwrap_or_default()
    }
}

/// Reads `[-]d[.ddd]e[-]x` text into a [`Decimal`], trailing zeros trimmed
/// down to one digit.
fn decimal(text: &[u8]) -> Decimal {
    let mut out = Decimal {
        digits: [0; 24],
        len: 0,
        exponent: 0,
    };
    let (mantissa, exponent) = match text.iter().position(|b| *b == b'e') {
        Some(e) => (
            text.get(..e).unwrap_or_default(),
            text.get(e.saturating_add(1)..).unwrap_or_default(),
        ),
        None => (text, &b"0"[..]),
    };
    out.exponent = std::str::from_utf8(exponent)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    for b in mantissa.iter().filter(|b| b.is_ascii_digit()) {
        if let Some(slot) = out.digits.get_mut(out.len) {
            *slot = *b;
            out.len = out.len.saturating_add(1);
        }
    }
    // The first digit is never trimmed: past it, up to the last non-zero.
    let rest = out.digits.get(1..out.len).unwrap_or_default();
    out.len = rest
        .iter()
        .rposition(|b| *b != b'0')
        .map_or(1, |last| last.saturating_add(2));
    out
}

/// `format_args` written into `buf`.
fn formatted<'b>(buf: &'b mut [u8], args: std::fmt::Arguments<'_>) -> io::Result<&'b [u8]> {
    let mut cursor = io::Cursor::new(&mut buf[..]);
    cursor.write_fmt(args)?;
    let len = usize::try_from(cursor.position()).unwrap_or(0);
    Ok(buf.get(..len).unwrap_or_default())
}

/// Writes `d` as `Double.toString(d)` does.
pub fn write_java_double(out: &mut dyn Write, d: f64) -> io::Result<()> {
    if d.is_nan() {
        return out.write_all(b"NaN");
    }
    if d.is_infinite() {
        return out.write_all(if d.is_sign_positive() {
            b"Infinity"
        } else {
            b"-Infinity"
        });
    }
    if d == 0.0 {
        return out.write_all(if d.is_sign_negative() {
            b"-0.0"
        } else {
            b"0.0"
        });
    }
    // The longest text is a sign, 17 digits, a point, `e`, a sign and 3
    // exponent digits.
    let mut buf = [0u8; 40];
    let mut dec = decimal(formatted(&mut buf, format_args!("{d:e}"))?);
    if dec.len == 1 {
        // Java considers two-digit decimals too and keeps the closest; the
        // correctly rounded two digits are that one, and read back as `d`
        // since they are no further from it than the one digit.
        dec = decimal(formatted(&mut buf, format_args!("{d:.1e}"))?);
    }
    if d.is_sign_negative() {
        out.write_all(b"-")?;
    }
    let digits = dec.digits();
    if (1e-3..1e7).contains(&d.abs()) {
        return plain(out, digits, dec.exponent);
    }
    out.write_all(digits.get(..1).unwrap_or(b"0"))?;
    out.write_all(b".")?;
    let rest = digits.get(1..).unwrap_or_default();
    out.write_all(if rest.is_empty() { b"0" } else { rest })?;
    write!(out, "E{}", dec.exponent)
}

/// `digits * 10^exponent` (the first digit's) in plain notation, with at
/// least one digit after the point.
fn plain(out: &mut dyn Write, digits: &[u8], exponent: i32) -> io::Result<()> {
    if exponent < 0 {
        out.write_all(b"0.")?;
        for _ in 1..exponent.unsigned_abs() {
            out.write_all(b"0")?;
        }
        return out.write_all(digits);
    }
    let int_len = usize::try_from(exponent).unwrap_or(0).saturating_add(1);
    if int_len >= digits.len() {
        out.write_all(digits)?;
        for _ in digits.len()..int_len {
            out.write_all(b"0")?;
        }
        return out.write_all(b".0");
    }
    out.write_all(digits.get(..int_len).unwrap_or_default())?;
    out.write_all(b".")?;
    out.write_all(digits.get(int_len..).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn java(d: f64) -> String {
        let mut out = Vec::new();
        write_java_double(&mut out, d).expect("writes");
        String::from_utf8(out).expect("utf-8")
    }

    #[test]
    fn doubles_print_as_java_prints_them() {
        // Expected strings are what Java 21's `Double.toString` returns.
        assert_eq!(java(0.0), "0.0");
        assert_eq!(java(-0.0), "-0.0");
        assert_eq!(java(1.0), "1.0");
        assert_eq!(java(-2.5), "-2.5");
        assert_eq!(java(2002.8), "2002.8");
        assert_eq!(java(10014.0), "10014.0");
        assert_eq!(java(0.001), "0.001");
        assert_eq!(java(0.000_999), "9.99E-4");
        assert_eq!(java(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(java(9_999_999.0), "9999999.0");
        assert_eq!(java(1e7), "1.0E7");
        assert_eq!(java(9.835_776e11), "9.835776E11");
        assert_eq!(java(1.23e-5), "1.23E-5");
        assert_eq!(java(-1.5e300), "-1.5E300");
        assert_eq!(java(123_456.789), "123456.789");
        assert_eq!(java(0.012_5), "0.0125");
        assert_eq!(java(f64::MAX), "1.7976931348623157E308");
        assert_eq!(java(f64::MIN_POSITIVE), "2.2250738585072014E-308");
        // One digit would read back, but Java keeps the closer two.
        assert_eq!(java(f64::from_bits(1)), "4.9E-324");
        assert_eq!(java(f64::NAN), "NaN");
        assert_eq!(java(f64::INFINITY), "Infinity");
        assert_eq!(java(f64::NEG_INFINITY), "-Infinity");
    }
}
