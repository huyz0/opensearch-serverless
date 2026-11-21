//! How a compact manifest's body is compressed, by the id its header names.
//!
//! Ported from `ManifestCodec` (`decompress`): none, deflate without
//! framing, zstd, and Lucene's LZ4.

use std::borrow::Cow;

use crate::error::{FormatError, corrupt, len64};

const WHAT: &str = "bucket manifest";

/// The largest decompressed body a read accepts.
pub(super) const MAX_BODY_BYTES: usize = 64 << 20;

/// A zstd stream of run-length blocks expands about 43,000 times.
const ZSTD_MAX_RATIO: usize = 1 << 16;
/// LZ4 expands at most 255 times.
const LZ4_MAX_RATIO: usize = 255;

const NONE: u8 = 0;
const DEFLATE: u8 = 1;
const ZSTD: u8 = 2;
const LZ4_FAST: u8 = 3;

/// A codec a manifest is written with. LZ4 is read, not written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestCodec {
    /// The body as it is.
    None,
    /// Deflate without framing.
    Deflate,
    /// Zstandard.
    Zstd,
}

impl ManifestCodec {
    pub(super) const fn id(self) -> u8 {
        match self {
            Self::None => NONE,
            Self::Deflate => DEFLATE,
            Self::Zstd => ZSTD,
        }
    }
}

/// Refuses a body the reader would refuse.
pub(super) fn check_body_length(length: usize) -> Result<(), FormatError> {
    if length > MAX_BODY_BYTES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: len64(MAX_BODY_BYTES),
            actual: len64(length),
        });
    }
    Ok(())
}

/// Compresses a body with a codec.
pub(super) fn compress(codec: ManifestCodec, raw: &[u8]) -> Result<Vec<u8>, FormatError> {
    match codec {
        // ALLOC: a copy of the body, at most MAX_BODY_BYTES (checked by the caller).
        ManifestCodec::None => Ok(raw.to_vec()),
        ManifestCodec::Deflate => Ok(miniz_oxide::deflate::compress_to_vec(raw, 6)),
        ManifestCodec::Zstd => {
            // ALLOC: the compressed body, at most the compressor's bound for a body under MAX_BODY_BYTES.
            let mut out = vec![0u8; zstd_safe::compress_bound(raw.len())];
            let written = zstd_safe::compress(out.as_mut_slice(), raw, 3)
                .map_err(|_| corrupt(WHAT, "a zstd body could not be compressed"))?;
            out.truncate(written);
            Ok(out)
        }
    }
}

/// The body a header's codec and length describe. Uncompressed bodies are
/// borrowed where they lie.
pub(super) fn body(
    codec: u8,
    packed: &[u8],
    raw_length: usize,
) -> Result<Cow<'_, [u8]>, FormatError> {
    if raw_length > MAX_BODY_BYTES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: len64(MAX_BODY_BYTES),
            actual: len64(raw_length),
        });
    }
    let raw = match codec {
        NONE if packed.len() == raw_length => return Ok(Cow::Borrowed(packed)),
        NONE => return Err(corrupt(WHAT, "a body is not the length its header says")),
        // Neither codec expands past these ratios, so a header claiming more
        // than the stream can hold is refused before the body is allocated.
        ZSTD if raw_length > packed.len().saturating_mul(ZSTD_MAX_RATIO) => {
            return Err(corrupt(WHAT, "a header claims more than its stream holds"));
        }
        LZ4_FAST if raw_length > packed.len().saturating_mul(LZ4_MAX_RATIO) => {
            return Err(corrupt(WHAT, "a header claims more than its stream holds"));
        }
        DEFLATE => miniz_oxide::inflate::decompress_to_vec_with_limit(packed, raw_length)
            .map_err(|_| corrupt(WHAT, "a deflate body is malformed"))?,
        ZSTD => zstd(packed, raw_length)?,
        LZ4_FAST => lz4(packed, raw_length)?,
        other => {
            return Err(FormatError::Unsupported {
                what: "bucket manifest codec",
                version: u64::from(other),
            });
        }
    };
    if raw.len() != raw_length {
        return Err(corrupt(WHAT, "a body is not the length its header says"));
    }
    Ok(Cow::Owned(raw))
}

fn zstd(packed: &[u8], raw_length: usize) -> Result<Vec<u8>, FormatError> {
    // ALLOC: the body, at most MAX_BODY_BYTES (checked by the caller).
    let mut raw = vec![0u8; raw_length];
    let written = zstd_safe::decompress(raw.as_mut_slice(), packed)
        .map_err(|_| corrupt(WHAT, "a zstd body is malformed"))?;
    raw.truncate(written);
    Ok(raw)
}

/// Lucene's `LZ4.decompress`: token, literals, a two-byte back offset, match.
fn lz4(packed: &[u8], raw_length: usize) -> Result<Vec<u8>, FormatError> {
    const BAD: FormatError = corrupt(WHAT, "an LZ4 body is malformed");
    // ALLOC: the body, at most MAX_BODY_BYTES (checked by the caller).
    let mut out: Vec<u8> = Vec::with_capacity(raw_length);
    let mut input = packed.iter().copied();
    while out.len() < raw_length {
        let token = input.next().ok_or(BAD)?;
        let mut literals = usize::from(token >> 4);
        if literals == 0x0F {
            literals = literals.saturating_add(extended(&mut input).ok_or(BAD)?);
        }
        if literals > raw_length.saturating_sub(out.len()) {
            return Err(BAD);
        }
        for _ in 0..literals {
            out.push(input.next().ok_or(BAD)?);
        }
        if out.len() >= raw_length {
            break;
        }
        let low = usize::from(input.next().ok_or(BAD)?);
        let high = usize::from(input.next().ok_or(BAD)?);
        let back = low | (high << 8);
        if back == 0 || back > out.len() {
            return Err(BAD);
        }
        let mut length = usize::from(token & 0x0F);
        if length == 0x0F {
            length = length.saturating_add(extended(&mut input).ok_or(BAD)?);
        }
        length = length.saturating_add(4);
        if length > raw_length.saturating_sub(out.len()) {
            return Err(BAD);
        }
        let start = out.len().saturating_sub(back);
        for i in 0..length {
            // A match may overlap what it copies, so each byte is read after the last is pushed.
            let byte = *out.get(start.saturating_add(i)).ok_or(BAD)?;
            out.push(byte);
        }
    }
    Ok(out)
}

/// The run of 0xFF bytes and the byte that ends it, summed.
fn extended(input: &mut impl Iterator<Item = u8>) -> Option<usize> {
    let mut total = 0usize;
    loop {
        let b = input.next()?;
        total = total.saturating_add(usize::from(b));
        if b != 0xFF {
            return Some(total);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_may_end_with_a_match_that_fills_the_body_exactly() {
        // Four literals, then a match four bytes back of length four.
        let packed = [0x40, b'a', b'b', b'c', b'd', 4, 0];
        assert_eq!(lz4(&packed, 8).unwrap(), b"abcdabcd");
        // A match may overlap what it copies: one literal, copied seven times.
        assert_eq!(lz4(&[0x13, b'x', 1, 0], 8).unwrap(), b"xxxxxxxx");
    }

    #[test]
    fn a_match_past_the_body_or_a_missing_token_is_refused() {
        let packed = [0x40, b'a', b'b', b'c', b'd', 4, 0];
        assert!(lz4(&packed, 7).is_err());
        assert!(lz4(&packed, 9).is_err());
    }

    #[test]
    fn the_cap_and_the_stream_ratio_refuse_a_header_before_the_body_is_allocated() {
        // At the cap the refusal is the ratio's (corrupt); past it, the cap's.
        assert!(matches!(
            body(ZSTD, b"xxxx", MAX_BODY_BYTES),
            Err(FormatError::Corrupt { .. })
        ));
        assert!(matches!(
            body(ZSTD, b"xxxx", MAX_BODY_BYTES + 1),
            Err(FormatError::TooLarge { .. })
        ));
        assert!(body(NONE, b"abc", 3).is_ok());
        assert!(body(NONE, b"abc", 2).is_err());
        assert!(body(LZ4_FAST, &[0u8; 2], 511).is_err());
        assert!(
            body(LZ4_FAST, &[0u8; 2], 510).is_err(),
            "not an lz4 stream, but past the ratio check"
        );
    }

    #[test]
    fn a_long_literal_run_and_a_long_match_use_the_extension_bytes() {
        // 300 literals: 15 in the token, then 255 and 30.
        let mut packed = vec![0xF0, 255, 30];
        packed.extend(std::iter::repeat_n(7u8, 300));
        assert_eq!(lz4(&packed, 300).unwrap(), vec![7u8; 300]);
        // Then a match of 4 + 15 + 255 + 6 = 280 bytes back 1.
        packed.truncate(3 + 300);
        packed[0] = 0xFF;
        packed.extend([1, 0, 255, 6]);
        assert_eq!(lz4(&packed, 580).unwrap(), vec![7u8; 580]);
    }
}
