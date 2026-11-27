//! How a response prints the values every hit carries: Java's float text, a
//! document's `_id` and a score.

use std::io::{self, Write};

use oss_rest::write_json_str;

use crate::search::SortValue;

/// `Float.toString`: the shortest digits that read back as the float, in
/// plain notation from 10^-3 up to 10^7 (always with a fraction) and in
/// computerized scientific notation outside it.
pub fn write_java_float(out: &mut dyn Write, f: f32) -> io::Result<()> {
    if f.is_nan() {
        return out.write_all(b"NaN");
    }
    if f.is_infinite() {
        return out.write_all(if f > 0.0 { b"Infinity" } else { b"-Infinity" });
    }
    let magnitude = f.abs();
    if magnitude == 0.0 || (1e-3..1e7).contains(&magnitude) {
        // A fixed buffer: the longest plain float is a sign, seven integer
        // digits, a point and nine fraction digits.
        let mut buf = [0u8; 32];
        let mut cursor = io::Cursor::new(&mut buf[..]);
        write!(cursor, "{f}")?;
        let len = usize::try_from(cursor.position()).unwrap_or(0);
        let text = buf.get(..len).unwrap_or_default();
        out.write_all(text)?;
        if !text.contains(&b'.') {
            out.write_all(b".0")?;
        }
        return Ok(());
    }
    let mut buf = [0u8; 32];
    let mut cursor = io::Cursor::new(&mut buf[..]);
    write!(cursor, "{f:e}")?;
    let len = usize::try_from(cursor.position()).unwrap_or(0);
    let text = buf.get(..len).unwrap_or_default();
    let (mantissa, exponent) = match text.iter().position(|b| *b == b'e') {
        Some(e) => (
            text.get(..e).unwrap_or_default(),
            text.get(e.saturating_add(1)..).unwrap_or_default(),
        ),
        None => (text, &b"0"[..]),
    };
    out.write_all(mantissa)?;
    if !mantissa.contains(&b'.') {
        out.write_all(b".0")?;
    }
    out.write_all(b"E")?;
    out.write_all(exponent)
}

const BASE64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// `Uid.decodeId`, written as a JSON string: a numeric id is two decimal
/// digits a byte (a final `f` nibble is padding), a UTF-8 id its bytes, and
/// any other id URL-safe base64 without padding.
pub fn write_id(out: &mut dyn Write, id: &[u8]) -> io::Result<()> {
    match id.split_first() {
        Some((0xfe, digits)) => {
            out.write_all(b"\"")?;
            let last = digits.len().saturating_sub(1);
            for (i, b) in digits.iter().enumerate() {
                out.write_all(&[b'0'.saturating_add(b >> 4)])?;
                let low = b & 0x0f;
                if i == last && low == 0x0f {
                    break;
                }
                out.write_all(&[b'0'.saturating_add(low)])?;
            }
            out.write_all(b"\"")
        }
        Some((0xff, text)) => write_json_str(out, &String::from_utf8_lossy(text)),
        Some((first, rest)) => {
            let bytes = if *first == 0xfd { rest } else { id };
            out.write_all(b"\"")?;
            for chunk in bytes.chunks(3) {
                let b = [
                    chunk.first().copied().unwrap_or(0),
                    chunk.get(1).copied().unwrap_or(0),
                    chunk.get(2).copied().unwrap_or(0),
                ];
                let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
                let chars = chunk.len().saturating_add(1);
                for k in 0..chars {
                    let shift =
                        18u32.saturating_sub(6u32.saturating_mul(u32::try_from(k).unwrap_or(0)));
                    let index = usize::try_from((n >> shift) & 0x3f).unwrap_or(0);
                    out.write_all(&[BASE64_URL.get(index).copied().unwrap_or(b'A')])?;
                }
            }
            out.write_all(b"\"")
        }
        None => out.write_all(b"\"\""),
    }
}

pub(super) fn write_score(out: &mut dyn Write, score: Option<f32>) -> io::Result<()> {
    match score {
        Some(s) => write_java_float(out, s),
        None => out.write_all(b"null"),
    }
}

/// Writes `,"sort":[...]` for a hit that has sort values.
pub(super) fn write_sort(out: &mut dyn Write, sort: &[SortValue]) -> io::Result<()> {
    if sort.is_empty() {
        return Ok(());
    }
    out.write_all(b",\"sort\":[")?;
    for (k, value) in sort.iter().enumerate() {
        if k > 0 {
            out.write_all(b",")?;
        }
        match value {
            SortValue::Long(n) => write!(out, "{n}")?,
            SortValue::Score(s) => write_java_float(out, *s)?,
            SortValue::Float(f) => write_java_float(out, *f)?,
            SortValue::Double(d) => out.write_all(crate::template::java_double(*d).as_bytes())?,
            SortValue::Term(Some(t)) => write_json_str(out, &String::from_utf8_lossy(t))?,
            SortValue::Term(None) => out.write_all(b"null")?,
        }
    }
    out.write_all(b"]")
}
