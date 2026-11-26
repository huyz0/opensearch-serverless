//! The stock envelope's writer: the counterpart of [`super::to_json`].
//!
//! A header naming the codec at version 1, the document as plain SMILE, and
//! the footer whose CRC covers every byte before it. The body is not
//! deflate-framed: the reader takes either, and a stock node reads this one.

use super::{EnvelopeLimits, FOOTER_MAGIC, HEADER_MAGIC, VERSION, WHAT};
use crate::error::{FormatError, corrupt};
use crate::smile;

/// The longest name whose length is one byte of the header's 7-bit length.
const MAX_ONE_BYTE_NAME: usize = 128;

/// Writes `json` after `out`'s end as a blob of codec `codec`; a failed call
/// leaves `out` as it was.
pub fn from_json(
    json: &[u8],
    codec: &str,
    limits: &EnvelopeLimits,
    out: &mut Vec<u8>,
) -> Result<(), FormatError> {
    if codec.is_empty() || codec.len() >= MAX_ONE_BYTE_NAME {
        return Err(corrupt(WHAT, "the codec name is not a usable length"));
    }
    let base = out.len();
    let done = write(json, codec, limits, out, base);
    if done.is_err() {
        out.truncate(base);
    }
    done
}

fn write(
    json: &[u8],
    codec: &str,
    limits: &EnvelopeLimits,
    out: &mut Vec<u8>,
    base: usize,
) -> Result<(), FormatError> {
    out.extend_from_slice(&HEADER_MAGIC);
    // A name under 128 bytes has a one-byte length.
    out.push(u8::try_from(codec.len()).unwrap_or(0));
    out.extend_from_slice(codec.as_bytes());
    out.extend_from_slice(&VERSION.to_be_bytes());
    smile::from_json(json, &limits.smile, out)?;
    out.extend_from_slice(&FOOTER_MAGIC);
    out.extend_from_slice(&[0u8; 4]);
    let crc = crc32fast::hash(out.get(base..).unwrap_or_default());
    out.extend_from_slice(&u64::from(crc).to_be_bytes());
    Ok(())
}
