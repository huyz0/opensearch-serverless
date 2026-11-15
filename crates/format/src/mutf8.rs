//! Java's modified UTF-8, as `DataOutput.writeUTF` writes it.
//!
//! Ported from `java.io.DataInputStream.readUTF`. It differs from UTF-8 in two
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

fn continuation(byte: Option<&u8>) -> Option<u16> {
    match byte {
        Some(&b) if b & 0xC0 == 0x80 => Some(u16::from(b & 0x3F)),
        _ => None,
    }
}
