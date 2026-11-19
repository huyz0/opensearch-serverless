//! URL-safe base64 without padding, the shape of a `next_token`.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Encodes `bytes` without padding.
pub(super) fn encode(bytes: &[u8]) -> String {
    // ALLOC: one token, bounded by a name.
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let b = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        for shift in [18u32, 12, 6, 0]
            .into_iter()
            .take(chunk.len().saturating_add(1))
        {
            out.push(char::from(
                ALPHABET[usize::try_from((n >> shift) & 63).unwrap_or(0)],
            ));
        }
    }
    out
}

/// Decodes `text`, with or without its padding; nothing for a character
/// outside the alphabet, a misplaced or wrong padding, or a length no
/// encoding has.
pub(super) fn decode(text: &str) -> Option<Vec<u8>> {
    let body = match text.strip_suffix("==") {
        Some(rest) if text.len().is_multiple_of(4) => rest,
        _ => match text.strip_suffix('=') {
            Some(rest) if text.len().is_multiple_of(4) => rest,
            Some(_) => return None,
            None => text,
        },
    };
    if body.len() % 4 == 1 {
        return None;
    }
    // ALLOC: one buffer, bounded by the token.
    let mut out = Vec::with_capacity(body.len());
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in body.bytes() {
        let v = ALPHABET.iter().position(|a| *a == c)?;
        acc = acc.wrapping_shl(6).wrapping_add(u32::try_from(v).ok()?);
        bits = bits.saturating_add(6);
        if bits >= 8 {
            bits = bits.saturating_sub(8);
            out.push(u8::try_from((acc >> bits) & 0xFF).ok()?);
        }
    }
    Some(out)
}
