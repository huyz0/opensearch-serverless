//! A stock OpenSearch repository's metadata blobs.
//!
//! Ported from `ChecksumBlobStoreFormat` and Lucene's `CodecUtil`: a header
//! (the magic, the codec name and version 1), a SMILE body that may be
//! deflate-framed (`DeflateCompressor`), and a footer whose CRC covers
//! everything before it. The checksum is verified before the body is read.

mod index_metadata;
mod repository_data;
mod shard_snapshot;

use crate::error::{FormatError, corrupt, len64};
use crate::smile::{self, Limits as SmileLimits};

pub use index_metadata::{MAX_ALIASES, MAX_SETTINGS, StockIndexMetadata};
pub use repository_data::{RepositoryData, StockIndex, StockSnapshot, is_blob_name};
pub use shard_snapshot::{MAX_CHUNKS, MAX_FILES, StockFile, StockShardSnapshot};

const WHAT: &str = "stock repository blob";

/// The first four bytes of a Lucene codec header.
const HEADER_MAGIC: [u8; 4] = [0x3F, 0xD7, 0x6C, 0x17];
/// The footer's magic, the header's bitwise complement.
const FOOTER_MAGIC: [u8; 4] = [0xC0, 0x28, 0x93, 0xE8];
/// The footer: magic, algorithm id and the CRC as a long.
const FOOTER_BYTES: usize = 16;
/// The only version the stock format has written.
const VERSION: u32 = 1;
/// The longest codec name read: the stock names are under 16 bytes.
const MAX_CODEC_BYTES: usize = 128;
/// The bytes `DeflateCompressor` writes before a raw deflate stream.
const DEFLATE_HEADER: [u8; 4] = *b"DFL\0";

/// What a blob may decode to.
#[derive(Debug, Clone, Copy)]
pub struct EnvelopeLimits {
    /// The most bytes of body after inflating.
    pub decompressed_bytes: usize,
    /// The bounds of the SMILE decode.
    pub smile: SmileLimits,
}

impl Default for EnvelopeLimits {
    fn default() -> Self {
        Self {
            decompressed_bytes: 64 << 20,
            smile: SmileLimits::default(),
        }
    }
}

/// The body of `blob` once its footer checksum and header have verified.
///
/// `codec` is the name the caller expects (`snapshot`, `metadata`,
/// `index-metadata`, `snapshots`).
pub fn verified_body<'a>(blob: &'a [u8], codec: &str) -> Result<&'a [u8], FormatError> {
    let end = blob
        .len()
        .checked_sub(FOOTER_BYTES)
        .ok_or_else(|| FormatError::Truncated {
            what: WHAT,
            needed: len64(FOOTER_BYTES),
            have: len64(blob.len()),
        })?;
    let footer = blob.get(end..).unwrap_or_default();
    if footer.get(..4) != Some(&FOOTER_MAGIC[..]) {
        return Err(corrupt(WHAT, "the footer magic is wrong"));
    }
    if footer.get(4..8) != Some(&[0u8; 4][..]) {
        return Err(corrupt(WHAT, "the checksum algorithm is not CRC32"));
    }
    let stored = footer
        .get(8..16)
        .and_then(|b| <[u8; 8]>::try_from(b).ok())
        .map_or(u64::MAX, u64::from_be_bytes);
    let summed = blob.len().saturating_sub(8);
    let actual = crc32fast::hash(blob.get(..summed).unwrap_or_default());
    if stored != u64::from(actual) {
        return Err(corrupt(WHAT, "the footer checksum does not match"));
    }
    let body_at = header_end(blob.get(..end).unwrap_or_default(), codec)?;
    Ok(blob.get(body_at..end).unwrap_or_default())
}

/// Where the body starts: past a header that names `codec` at version 1.
fn header_end(blob: &[u8], codec: &str) -> Result<usize, FormatError> {
    if blob.get(..4) != Some(&HEADER_MAGIC[..]) {
        return Err(corrupt(WHAT, "the header magic is wrong"));
    }
    // The codec name is a Lucene string: a little-endian 7-bit length, then
    // that many bytes.
    let (mut len, mut at, mut done) = (0usize, 4usize, false);
    // Three bytes carry 21 bits, far past the longest name read.
    for shift in [0u32, 7, 14] {
        let b = *blob.get(at).ok_or(FormatError::Truncated {
            what: WHAT,
            needed: 0,
            have: len64(blob.len()),
        })?;
        at = at.saturating_add(1);
        len |= usize::from(b & 0x7F).wrapping_shl(shift);
        if b & 0x80 == 0 {
            done = true;
            break;
        }
    }
    if !done {
        return Err(corrupt(WHAT, "the codec name length is too long"));
    }
    if len > MAX_CODEC_BYTES {
        return Err(corrupt(WHAT, "the codec name is too long"));
    }
    let name_end = at.saturating_add(len);
    let name = blob.get(at..name_end).ok_or(FormatError::Truncated {
        what: WHAT,
        needed: len64(name_end),
        have: len64(blob.len()),
    })?;
    if name != codec.as_bytes() {
        return Err(corrupt(WHAT, "the codec name is not the one expected"));
    }
    let version_end = name_end.saturating_add(4);
    let version = blob
        .get(name_end..version_end)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_be_bytes)
        .ok_or(FormatError::Truncated {
            what: WHAT,
            needed: len64(version_end),
            have: len64(blob.len()),
        })?;
    if version != VERSION {
        return Err(FormatError::Unsupported {
            what: WHAT,
            version: u64::from(version),
        });
    }
    Ok(version_end)
}

/// The JSON of a stock metadata blob: its checksum verified, its body
/// inflated up to the limit when framed, then decoded from SMILE.
pub fn to_json(
    blob: &[u8],
    codec: &str,
    limits: &EnvelopeLimits,
    out: &mut Vec<u8>,
) -> Result<(), FormatError> {
    let body = verified_body(blob, codec)?;
    match body.strip_prefix(&DEFLATE_HEADER[..]) {
        Some(packed) => {
            // ALLOC: at most `decompressed_bytes`; the inflater stops there.
            let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(
                packed,
                limits.decompressed_bytes,
            )
            .map_err(|e| match e.status {
                miniz_oxide::inflate::TINFLStatus::HasMoreOutput => FormatError::TooLarge {
                    what: WHAT,
                    limit: len64(limits.decompressed_bytes),
                    actual: len64(limits.decompressed_bytes).saturating_add(1),
                },
                _ => corrupt(WHAT, "the deflate body is malformed"),
            })?;
            smile::to_json(&raw, &limits.smile, out)
        }
        None => smile::to_json(body, &limits.smile, out),
    }
}
