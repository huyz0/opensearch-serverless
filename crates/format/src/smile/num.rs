//! The number and binary encodings of SMILE: 7-bit variable integers,
//! zigzag, floats as 7-bit groups, big integers, 7-bit binary, base64.

use super::Cursor;
use crate::error::{FormatError, corrupt, len64};

const WHAT: &str = "SMILE document";

/// A positive variable-length integer: 7 bits a byte, most significant
/// first, the last byte marked by its top bit and carrying 6 bits.
pub(super) fn vint(c: &mut Cursor<'_>) -> Result<u64, FormatError> {
    let mut acc: u128 = 0;
    for _ in 0..10 {
        let b = c.byte()?;
        if b & 0x80 != 0 {
            acc = (acc << 6) | u128::from(b & 0x3F);
            return u64::try_from(acc).map_err(|_| corrupt(WHAT, "an integer wider than 64 bits"));
        }
        acc = (acc << 7) | u128::from(b);
    }
    Err(corrupt(WHAT, "an integer of more than ten bytes"))
}

/// Zigzag decoding.
pub(super) fn unzigzag(n: u64) -> i64 {
    let magnitude = i64::try_from(n >> 1).unwrap_or(i64::MAX);
    if n & 1 == 0 { magnitude } else { !magnitude }
}

/// A float or double as 7-bit groups, `bytes` of them (5 or 10).
pub(super) fn float_bits(c: &mut Cursor<'_>, bytes: usize) -> Result<u64, FormatError> {
    let mut acc: u128 = 0;
    for _ in 0..bytes {
        let b = c.byte()?;
        if b & 0x80 != 0 {
            return Err(corrupt(WHAT, "a float byte with its top bit set"));
        }
        acc = (acc << 7) | u128::from(b);
    }
    u64::try_from(acc).map_err(|_| corrupt(WHAT, "a float wider than 64 bits"))
}

/// Decodes `n` bytes held as 7-bit groups: 7 bytes in 8 groups, then `r`
/// bytes in `r + 1`.
pub(super) fn decode7(c: &mut Cursor<'_>, n: usize) -> Result<Vec<u8>, FormatError> {
    let tail = n % 7;
    let units = (n / 7)
        .checked_mul(8)
        .and_then(|u| u.checked_add(if tail == 0 { 0 } else { tail.saturating_add(1) }))
        .ok_or_else(|| corrupt(WHAT, "a binary length that overflows"))?;
    // The group bytes are all present before anything is allocated.
    let groups = c.take(units)?;
    // ALLOC: n bytes, and `groups` holds at least n * 8 / 7 bytes of input.
    let mut out = Vec::with_capacity(n);
    let (mut acc, mut bits) = (0u32, 0u32);
    let last = units.saturating_sub(1);
    for (i, &g) in groups.iter().enumerate() {
        // The last group of a tail holds the bits left over, right-aligned:
        // one for each tail byte.
        let width = if tail != 0 && i == last {
            u32::try_from(tail).unwrap_or(7)
        } else {
            7
        };
        if u32::from(g) >> width != 0 {
            return Err(corrupt(WHAT, "a 7-bit group wider than its width"));
        }
        acc = (acc << width) | u32::from(g);
        bits = bits.saturating_add(width);
        while bits >= 8 && out.len() < n {
            bits = bits.saturating_sub(8);
            out.push(u8::try_from((acc >> bits) & 0xFF).unwrap_or(0));
            acc &= !(u32::MAX.wrapping_shl(bits));
        }
    }
    if out.len() != n {
        return Err(corrupt(WHAT, "a binary shorter than its length"));
    }
    Ok(out)
}

/// A big integer's two's-complement bytes as decimal text.
pub(super) fn decimal(be: &[u8]) -> String {
    let negative = be.first().is_some_and(|b| b & 0x80 != 0);
    // ALLOC: the integer's bytes, at most lim.bigint_bytes (checked by the caller).
    let mut mag: Vec<u8> = be.to_vec();
    if negative {
        // Negate: invert, add one.
        let mut carry = true;
        for b in mag.iter_mut().rev() {
            *b = !*b;
            if carry {
                let (v, c) = b.overflowing_add(1);
                *b = v;
                carry = c;
            }
        }
    }
    // Base 1e9 limbs, least significant first.
    // ALLOC: one limb for each nine digits of the integer, bounded by its bytes.
    let mut limbs: Vec<u32> = Vec::new();
    for &b in &mag {
        let mut carry = u64::from(b);
        for l in &mut limbs {
            let v = u64::from(*l).saturating_mul(256).saturating_add(carry);
            *l = u32::try_from(v % 1_000_000_000).unwrap_or(0);
            carry = v / 1_000_000_000;
        }
        while carry > 0 {
            limbs.push(u32::try_from(carry % 1_000_000_000).unwrap_or(0));
            carry /= 1_000_000_000;
        }
    }
    // ALLOC: the integer's digits, bounded by its bytes.
    let mut text = String::new();
    if negative {
        text.push('-');
    }
    match limbs.split_last() {
        None => text.push('0'),
        Some((top, rest)) => {
            // ALLOC: one limb's digits.
            text.push_str(&top.to_string());
            for l in rest.iter().rev() {
                // ALLOC: one limb's digits.
                text.push_str(&format!("{l:09}"));
            }
        }
    }
    text
}

/// A decimal with `scale` digits after the point, from the integer text
/// `unscaled`, as a JSON number.
pub(super) fn with_scale(unscaled: &str, scale: i64) -> String {
    let (sign, digits) = unscaled
        .strip_prefix('-')
        .map_or(("", unscaled), |d| ("-", d));
    if scale == 0 {
        // ALLOC: the integer's text, bounded by its bytes.
        return unscaled.to_owned();
    }
    if !(0..=1000).contains(&scale) {
        // ALLOC: the integer's text and an exponent.
        return format!("{unscaled}E{}", scale.saturating_neg());
    }
    let scale = usize::try_from(scale).unwrap_or(0);
    if digits.len() > scale {
        let (whole, frac) = digits.split_at(digits.len().saturating_sub(scale));
        // ALLOC: the integer's text and a point, bounded by its bytes and a scale of 1,000.
        format!("{sign}{whole}.{frac}")
    } else {
        // ALLOC: the integer's text and up to 1,000 zeros.
        format!(
            "{sign}0.{}{digits}",
            "0".repeat(scale.saturating_sub(digits.len()))
        )
    }
}

/// Standard base64 with padding.
pub(super) fn base64(bytes: &[u8], out: &mut String) {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in bytes.chunks(3) {
        let at = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = (at(0) << 16) | (at(1) << 8) | at(2);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                let six = usize::try_from((n >> shift) & 0x3F).unwrap_or(0);
                out.push(char::from(A.get(six).copied().unwrap_or(b'=')));
            } else {
                out.push('=');
            }
        }
    }
}

/// The encoded size of `n` bytes in base64.
pub(super) fn base64_len(n: usize) -> u64 {
    len64(n).div_ceil(3).saturating_mul(4)
}
