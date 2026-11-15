//! Writing JSON text: escaped strings and OpenSearch's byte-size strings.
//!
//! Strings are escaped the way Jackson's default generator escapes them: `"`,
//! `\` and the control characters below U+0020; everything else, `/` and
//! non-ASCII included, is written as is. Byte sizes follow
//! `org.opensearch.core.common.unit.ByteSizeValue#toString` at
//! huyz0/OpenSearch@49f238ed.

use std::io::{self, Write};

/// Writes `s` as a quoted, escaped JSON string.
pub(crate) fn write_str(out: &mut dyn Write, s: &str) -> io::Result<()> {
    out.write_all(b"\"")?;
    let bytes = s.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let escape: &[u8] = match b {
            b'"' => b"\\\"",
            b'\\' => b"\\\\",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            0x08 => b"\\b",
            0x0c => b"\\f",
            0..=0x1f => b"",
            _ => continue,
        };
        out.write_all(&bytes[start..i])?;
        if escape.is_empty() {
            out.write_all(&unicode_escape(b))?;
        } else {
            out.write_all(escape)?;
        }
        // ARITH: `i` indexes `bytes`, so `i + 1` is at most its length.
        #[allow(clippy::arithmetic_side_effects)]
        {
            start = i + 1;
        }
    }
    out.write_all(&bytes[start..])?;
    out.write_all(b"\"")
}

/// `\u00XX` for a control byte.
fn unicode_escape(b: u8) -> [u8; 6] {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    [
        b'\\',
        b'u',
        b'0',
        b'0',
        HEX[usize::from(b >> 4)],
        HEX[usize::from(b & 0xf)],
    ]
}

/// `ByteSizeValue#toString`: the largest unit not above the value, with the
/// first decimal truncated, not rounded (`Strings.format1Decimals`), and the
/// decimal dropped when it is zero: `512b`, `1kb`, `1.5mb`.
pub(crate) fn write_byte_size(out: &mut dyn Write, bytes: u64) -> io::Result<()> {
    const UNITS: [(u32, &str); 5] = [(50, "pb"), (40, "tb"), (30, "gb"), (20, "mb"), (10, "kb")];
    let Some(&(shift, suffix)) = UNITS.iter().find(|(shift, _)| bytes >> shift > 0) else {
        return write!(out, "{bytes}b");
    };
    // Integer arithmetic gives the same digits as Java's double division
    // followed by truncation, except where the double rounds up into the next
    // tenth; that needs a fraction within 2^-52 of the boundary, which an
    // integer byte count at these magnitudes never reaches below 2^53 bytes.
    let whole = bytes >> shift;
    let rem = bytes & ((1u64 << shift).wrapping_sub(1));
    let tenth = (u128::from(rem).wrapping_mul(10)) >> shift;
    if tenth == 0 {
        write!(out, "{whole}{suffix}")
    } else {
        write!(out, "{whole}.{tenth}{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).expect("vec writes never fail");
        String::from_utf8(out).expect("utf-8")
    }

    #[test]
    fn strings_escape_quotes_backslashes_and_controls_only() {
        assert_eq!(s(|o| write_str(o, "plain")), "\"plain\"");
        assert_eq!(
            s(|o| write_str(o, "a\"b\\c/d\n\r\t\u{8}\u{c}\u{1}\u{1f}é")),
            "\"a\\\"b\\\\c/d\\n\\r\\t\\b\\f\\u0001\\u001Fé\""
        );
        assert_eq!(s(|o| write_str(o, "")), "\"\"");
    }

    #[test]
    fn escaped_strings_parse_back_to_the_original() {
        let original = "x\u{0}\u{7f}\"\\\u{10ffff}";
        let text = s(|o| write_str(o, original));
        let back: String = serde_json::from_str(&text).expect("valid json");
        assert_eq!(back, original);
    }

    #[test]
    fn byte_sizes_match_opensearch() {
        let cases = [
            (0, "0b"),
            (1023, "1023b"),
            (1024, "1kb"),
            (1536, "1.5kb"),
            (1024 * 1024 - 1, "1023.9kb"),
            (1 << 20, "1mb"),
            (100 << 20, "100mb"),
            ((1 << 30) + (1 << 29), "1.5gb"),
            (1 << 40, "1tb"),
            (3 << 49, "1.5pb"),
        ];
        for (bytes, want) in cases {
            assert_eq!(s(|o| write_byte_size(o, bytes)), want, "{bytes}");
        }
    }
}
