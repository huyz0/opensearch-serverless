//! The bytes the writer puts out for strings and integers.
//!
//! Strings are escaped as Jackson's default generator escapes them: `"`, `\`,
//! `\b \f \n \r \t`, and `\u00XX` with upper-case hex for the other control
//! characters below U+0020. `/`, DEL and non-ASCII are written as is.

use crate::string::scan_plain;
use crate::writer::Output;

/// The escape for each byte below U+0020.
const CONTROL: [[u8; 6]; 32] = {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut table = [[0u8; 6]; 32];
    let mut b = 0;
    while b < 32 {
        table[b] = [b'\\', b'u', b'0', b'0', HEX[b >> 4], HEX[b & 0xf]];
        b += 1;
    }
    table
};

/// The escape for a byte that `scan_plain` stops at.
fn escape(b: u8) -> &'static [u8] {
    match b {
        b'"' => b"\\\"",
        b'\\' => b"\\\\",
        0x08 => b"\\b",
        0x09 => b"\\t",
        0x0a => b"\\n",
        0x0c => b"\\f",
        0x0d => b"\\r",
        _ => CONTROL.get(usize::from(b)).map_or(b"", |e| e.as_slice()),
    }
}

/// Writes `s` escaped, without quotes: each run of plain bytes in one write.
// ARITH: `j` is an index into `bytes`, so `j + 1` is at most its length.
#[allow(clippy::arithmetic_side_effects)]
pub(crate) fn escaped<O: Output>(s: &str, out: &mut O) -> Result<(), O::Error> {
    let bytes = s.as_bytes();
    let mut i = 0;
    // each pass passes at least one byte, so the loop is bounded
    for _ in 0..=bytes.len() {
        let j = scan_plain(bytes, i);
        if let Some(run) = bytes.get(i..j).filter(|run| !run.is_empty()) {
            out.write(run)?;
        }
        let Some(&b) = bytes.get(j) else { break };
        out.write(escape(b))?;
        i = j + 1;
    }
    Ok(())
}

/// The decimal digits of `n`, right-aligned in `buf`; returns where they start.
// ARITH: `at` starts at 20, the digit count of `u64::MAX`, and drops by one
// per digit; `n % 10` is below 10.
#[allow(clippy::arithmetic_side_effects)]
pub(crate) fn decimal(mut n: u64, buf: &mut [u8; 20]) -> usize {
    let mut at = buf.len();
    for slot in buf.iter_mut().rev() {
        *slot = b'0' + (n % 10) as u8;
        at -= 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_writes_every_digit_count() {
        let mut buf = [0u8; 20];
        for n in [0, 7, 10, 99, 100, 12_345, u64::from(u32::MAX), u64::MAX] {
            let at = decimal(n, &mut buf);
            assert_eq!(&buf[at..], n.to_string().as_bytes());
        }
    }

    #[test]
    fn every_stopping_byte_has_an_escape() {
        for b in (0u8..0x20).chain(*b"\"\\") {
            let e = escape(b);
            assert_eq!(e.first(), Some(&b'\\'), "{b:#x}");
            assert!(e.len() == 2 || e.len() == 6, "{b:#x}");
        }
    }
}
