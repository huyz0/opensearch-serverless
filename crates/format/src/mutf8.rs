//! Java's modified UTF-8, as `DataOutput.writeUTF` writes it.
//!
//! Ported from `java.io.DataInputStream.readUTF` and
//! `DataOutputStream.writeUTF`. It differs from UTF-8 in two
//! ways: NUL is the two bytes `C0 80`, and a character past U+FFFF is its two
//! UTF-16 surrogates, three bytes each. Text without either is plain UTF-8,
//! which is every id the shell writes, so it is borrowed as is.

use std::borrow::Cow;

/// Decodes `bytes`, borrowing when they are plain UTF-8; `None` for bytes
/// `readUTF` would refuse.
pub(crate) fn decode(bytes: &[u8]) -> Option<Cow<'_, str>> {
    if let Ok(plain) = std::str::from_utf8(bytes) {
        // Plain UTF-8 that is also valid modified UTF-8: no four-byte
        // sequence, which readUTF refuses.
        if !bytes.iter().any(|b| *b >= 0xF0) {
            return Some(Cow::Borrowed(plain));
        }
        return None;
    }
    // ALLOC: one string for a field that is not plain UTF-8, at most 65,535 bytes (a u16 length).
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len());
    let mut at = 0usize;
    while let Some(&first) = bytes.get(at) {
        let (unit, width) = match first {
            0x00..=0x7F => (u16::from(first), 1),
            0xC0..=0xDF => {
                let second = continuation(bytes.get(at.saturating_add(1)))?;
                ((u16::from(first & 0x1F) << 6) | second, 2)
            }
            0xE0..=0xEF => {
                let second = continuation(bytes.get(at.saturating_add(1)))?;
                let third = continuation(bytes.get(at.saturating_add(2)))?;
                ((u16::from(first & 0x0F) << 12) | (second << 6) | third, 3)
            }
            _ => return None,
        };
        units.push(unit);
        at = at.saturating_add(width);
    }
    // readUTF accepts lone surrogates; a Rust string cannot hold one.
    // ALLOC: the decoded string, at most three bytes a UTF-16 unit of a field under 65,536 bytes.
    char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .ok()
        .map(Cow::Owned)
}

/// The bytes `writeUTF` writes for `text` after its length.
pub(crate) fn encoded_len(text: &str) -> usize {
    text.chars()
        .map(|c| match u32::from(c) {
            0 => 2,
            1..=0x7F => 1,
            0x80..=0x7FF => 2,
            0x800..=0xFFFF => 3,
            _ => 6,
        })
        .fold(0usize, usize::saturating_add)
}

/// Writes `text` as `writeUTF` does: a big-endian `u16` length, then
/// modified UTF-8. `None`, writing nothing, past 65,535 bytes, where the
/// Java throws `UTFDataFormatException`.
pub(crate) fn write(text: &str, out: &mut Vec<u8>) -> Option<()> {
    let length = u16::try_from(encoded_len(text)).ok()?;
    out.extend_from_slice(&length.to_be_bytes());
    for c in text.chars() {
        let code = u32::from(c);
        if code != 0 && code <= 0x7F {
            out.push(c as u8);
        } else if code <= 0x7FF {
            // NUL lands here too, as C0 80.
            out.extend_from_slice(&[0xC0 | (code >> 6) as u8, 0x80 | (code & 0x3F) as u8]);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                let unit = u32::from(*unit);
                out.extend_from_slice(&[
                    0xE0 | (unit >> 12) as u8,
                    0x80 | ((unit >> 6) & 0x3F) as u8,
                    0x80 | (unit & 0x3F) as u8,
                ]);
            }
        }
    }
    Some(())
}

fn continuation(byte: Option<&u8>) -> Option<u16> {
    match byte {
        Some(&b) if b & 0xC0 == 0x80 => Some(u16::from(b & 0x3F)),
        _ => None,
    }
}
