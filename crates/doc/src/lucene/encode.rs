//! The byte and number encodings OpenSearch indexes with: `Uid.encodeId`,
//! `SmallFloat.intToByte4` (BM25's norm), `NumericUtils`' sortable floats,
//! and the points' sortable bytes.

use crate::mapping::MappingError;

const UTF8: u8 = 0xff;
const NUMERIC: u8 = 0xfe;
const BASE64_ESCAPE: u8 = 0xfd;

/// `Uid.encodeId`: a numeric id packed two digits a byte, a URL-safe base64
/// id decoded, anything else as UTF-8, each behind its marker byte.
///
/// # Errors
///
/// `Ids can't be empty`, as the Java's.
pub fn encode_id(id: &str) -> Result<Vec<u8>, MappingError> {
    if id.is_empty() {
        return Err(MappingError::illegal_argument("Ids can't be empty"));
    }
    if id.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(numeric_id(id.as_bytes()));
    }
    if let Some(decoded) = url_base64(id.as_bytes()) {
        return Ok(decoded);
    }
    // ALLOC: one id, bounded by the id's length (512 bytes, the index request's limit).
    let mut out = Vec::with_capacity(id.len().saturating_add(1));
    out.push(UTF8);
    out.extend_from_slice(id.as_bytes());
    Ok(out)
}

fn numeric_id(digits: &[u8]) -> Vec<u8> {
    // ALLOC: one id, bounded by the id's length.
    let mut out = Vec::with_capacity(digits.len().div_ceil(2).saturating_add(1));
    out.push(NUMERIC);
    for pair in digits.chunks(2) {
        let high = pair.first().map_or(0, |d| d.wrapping_sub(b'0'));
        let low = pair.get(1).map_or(0x0f, |d| d.wrapping_sub(b'0'));
        out.push((high << 4) | low);
    }
    out
}

/// The 6-bit value of a URL-safe base64 symbol.
fn sextet(c: u8) -> Option<u8> {
    Some(match c {
        b'A'..=b'Z' => c.wrapping_sub(b'A'),
        b'a'..=b'z' => c.wrapping_sub(b'a').wrapping_add(26),
        b'0'..=b'9' => c.wrapping_sub(b'0').wrapping_add(52),
        b'-' => 62,
        b'_' => 63,
        _ => return None,
    })
}

/// `isURLBase64WithoutPadding` and `encodeBase64Id`: the decoded bytes,
/// behind the escape byte when the first would read as a marker; `None`
/// when the id is not such base64. A last symbol must leave its unused
/// bits zero, as the Java's check of the last symbol requires.
fn url_base64(id: &[u8]) -> Option<Vec<u8>> {
    let tail = id.len() % 4;
    if tail == 1 {
        return None;
    }
    // ALLOC: one id, bounded by the id's length.
    let mut out = Vec::with_capacity(id.len().saturating_add(1));
    out.push(BASE64_ESCAPE);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in id {
        acc = (acc << 6) | u32::from(sextet(c)?);
        bits = bits.saturating_add(6);
        if bits >= 8 {
            bits = bits.saturating_sub(8);
            out.push(u8::try_from((acc >> bits) & 0xff).unwrap_or(0));
        }
    }
    // `bits` is under 8 here: the bits a last symbol carries past its bytes.
    if acc & (1u32 << bits).wrapping_sub(1) != 0 {
        return None;
    }
    if out.get(1).is_some_and(|b| *b < BASE64_ESCAPE) {
        out.remove(0);
    }
    Some(out)
}

/// `SmallFloat.longToInt4`.
fn long_to_int4(i: u64) -> u32 {
    let bits = 64u32.saturating_sub(i.leading_zeros());
    if bits < 4 {
        return u32::try_from(i).unwrap_or(0);
    }
    let shift = bits.saturating_sub(4);
    let encoded = u32::try_from((i >> shift) & 0x07).unwrap_or(0);
    encoded | (shift.saturating_add(1) << 3)
}

/// `SmallFloat.NUM_FREE_VALUES`: 255 less `longToInt4(Integer.MAX_VALUE)`.
const NUM_FREE_VALUES: u32 = 24;

/// `SmallFloat.intToByte4` as `BM25Similarity.computeNorm` returns it: the
/// byte, sign-extended to a long.
/// A length past `Integer.MAX_VALUE`, which the Java's `int` cannot
/// hold, reads as that.
pub fn norm(tokens: u32) -> i64 {
    let tokens = tokens.min(0x7fff_ffff);
    let byte = if tokens < NUM_FREE_VALUES {
        tokens
    } else {
        NUM_FREE_VALUES.saturating_add(long_to_int4(u64::from(
            tokens.saturating_sub(NUM_FREE_VALUES),
        )))
    };
    i64::from(i8::from_ne_bytes([u8::try_from(byte).unwrap_or(u8::MAX)]))
}

/// `NumericUtils.floatToSortableInt`.
pub fn float_to_sortable_int(f: f32) -> i32 {
    let bits = i32::from_ne_bytes(f.to_bits().to_ne_bytes());
    bits ^ ((bits >> 31) & 0x7fff_ffff)
}

/// `NumericUtils.doubleToSortableLong`.
pub fn double_to_sortable_long(d: f64) -> i64 {
    let bits = i64::from_ne_bytes(d.to_bits().to_ne_bytes());
    bits ^ ((bits >> 63) & 0x7fff_ffff_ffff_ffff)
}

/// `NumericUtils.intToSortableBytes`: an `IntPoint` or `FloatPoint`'s bytes.
pub fn int_point(v: i32) -> Vec<u8> {
    // ALLOC: one point, 4 bytes.
    (v ^ i32::MIN).to_be_bytes().to_vec()
}

/// `NumericUtils.shortToSortableBytes`: a `HalfFloatPoint`'s bytes.
pub fn short_point(v: i16) -> Vec<u8> {
    // ALLOC: one point, 2 bytes.
    (v ^ i16::MIN).to_be_bytes().to_vec()
}

/// `NumericUtils.longToSortableBytes`: a `LongPoint` or `DoublePoint`'s bytes.
pub fn long_point(v: i64) -> Vec<u8> {
    // ALLOC: one point, 8 bytes.
    (v ^ i64::MIN).to_be_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norms_are_small_float_bytes() {
        assert_eq!(norm(0), 0);
        assert_eq!(norm(7), 7);
        assert_eq!(norm(23), 23);
        assert_eq!(norm(24), 24);
        assert_eq!(norm(25), 25);
        assert_eq!(norm(31), 31);
        assert_eq!(norm(32), 32);
        assert_eq!(norm(40), 40);
        // 24 free values, then four significant bits of what is left.
        assert_eq!(norm(100), 57);
        assert_eq!(norm(1_000), 87);
        // The byte is signed: past 127 the norm reads negative.
        assert_eq!(norm(2_147_483_647), -1);
        assert_eq!(norm(u32::MAX), -1);
    }

    #[test]
    fn sortable_numbers_order_as_the_numbers() {
        assert_eq!(float_to_sortable_int(1.25), 1_067_450_368);
        assert_eq!(float_to_sortable_int(-1.0), -1_065_353_217);
        assert_eq!(
            double_to_sortable_long(-2.5e-10),
            -4_463_399_334_375_249_558
        );
        assert_eq!(double_to_sortable_long(0.1), 4_591_870_180_066_957_722);
        assert_eq!(int_point(-1), vec![0x7f, 0xff, 0xff, 0xff]);
        assert_eq!(long_point(1), vec![0x80, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn ids_encode_as_uid_encodes_them() {
        assert_eq!(encode_id("1").unwrap(), vec![0xfe, 0x1f]);
        assert_eq!(encode_id("01").unwrap(), vec![0xfe, 0x01]);
        assert_eq!(encode_id("abc").unwrap(), vec![0x69, 0xb7]);
        assert_eq!(encode_id("_-AB").unwrap(), vec![0xfd, 0xff, 0xe0, 0x01]);
        // A last symbol with bits past the decoded bytes is not base64.
        assert_eq!(encode_id("abd").unwrap(), b"\xffabd".to_vec());
        assert_eq!(encode_id("aB").unwrap(), b"\xffaB".to_vec());
        assert_eq!(encode_id("aQ").unwrap(), vec![0x69]);
        assert_eq!(encode_id("ABCDE").unwrap(), b"\xffABCDE".to_vec());
        assert_eq!(encode_id("a+b").unwrap(), b"\xffa+b".to_vec());
        assert_eq!(encode_id("").unwrap_err().reason, "Ids can't be empty");
    }
}
