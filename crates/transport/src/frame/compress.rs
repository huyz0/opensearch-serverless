//! A message's compression: OpenSearch's `DeflateCompressor`, raw deflate
//! behind the marker `DFL\0`.

use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZFlush, MZStatus};

use crate::error::WireError;

/// The marker before a compressed message.
pub const MARKER: [u8; 4] = *b"DFL\0";

/// The Java's level.
const LEVEL: u8 = 3;

/// The bytes inflated at a time.
const CHUNK: usize = 64 * 1024;

/// Compresses `message` after the marker, as the Java writes it.
pub fn deflate(message: &[u8]) -> Vec<u8> {
    let compressed = miniz_oxide::deflate::compress_to_vec(message, LEVEL);
    // ALLOC: the compressed message, at most a little more than the message.
    let mut out = Vec::with_capacity(compressed.len().saturating_add(MARKER.len()));
    out.extend_from_slice(&MARKER);
    out.extend_from_slice(&compressed);
    out
}

/// Inflates a compressed message, refusing output past `limit` bytes.
///
/// # Errors
///
/// [`WireError::Malformed`] without the marker or for a broken or
/// unfinished deflate stream; [`WireError::TooLarge`] when the output would
/// pass `limit`.
pub fn inflate_bounded(compressed: &[u8], limit: usize) -> Result<Vec<u8>, WireError> {
    const WHAT: &str = "a compressed message";
    let body = compressed
        .strip_prefix(&MARKER[..])
        .ok_or(WireError::malformed(
            WHAT,
            "a compressed message starts with DFL\\0",
        ))?;
    let mut state = InflateState::new_boxed(DataFormat::Raw);
    // ALLOC: grows a chunk at a time, never past `limit`.
    let mut out = Vec::new();
    let mut input = body;
    loop {
        let start = out.len();
        let room = limit.saturating_sub(start);
        // One byte past the limit shows output that would not fit.
        let grow = CHUNK.min(room.saturating_add(1));
        out.resize(start.saturating_add(grow), 0);
        let result = inflate(
            &mut state,
            input,
            out.get_mut(start..).unwrap_or_default(),
            MZFlush::None,
        );
        input = input.get(result.bytes_consumed..).unwrap_or_default();
        out.truncate(start.saturating_add(result.bytes_written));
        if out.len() > limit {
            return Err(WireError::too_large(WHAT, limit, out.len()));
        }
        match result.status {
            Ok(MZStatus::StreamEnd) => {
                return if input.is_empty() {
                    Ok(out)
                } else {
                    Err(WireError::malformed(
                        WHAT,
                        "bytes follow the deflate stream",
                    ))
                };
            }
            // miniz answers `Buf` when it can make no progress, so this loop ends.
            Ok(_) => {}
            Err(_) => {
                return Err(WireError::malformed(
                    WHAT,
                    "a broken or unfinished deflate stream",
                ));
            }
        }
    }
}
