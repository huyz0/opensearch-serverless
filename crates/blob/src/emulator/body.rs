//! The emulator's body handling: streaming a PUT into memory and selecting
//! a GET's bytes.

use crate::chunk::ChunkSource;
use crate::error::{BlobError, BlobResult};
use crate::store::ByteRange;

/// Reads exactly `len` bytes of `body` through `buf` into a new allocation;
/// a body that ends early or runs past `len` is refused.
pub(super) fn stream(body: &mut dyn ChunkSource, buf: &mut [u8], len: u64) -> BlobResult<Vec<u8>> {
    let cap = usize::try_from(len).map_err(|_| BlobError::CapacityExceeded)?;
    // ALLOC: one object body, at most the emulator's capacity, reserved by the caller.
    let mut data = Vec::with_capacity(cap);
    while data.len() < cap {
        let n = body.fill(buf)?;
        if n == 0 {
            return Err(BlobError::InvalidArgument(
                "body shorter than its declared length",
            ));
        }
        if data.len().saturating_add(n) > cap {
            return Err(BlobError::InvalidArgument(
                "body longer than its declared length",
            ));
        }
        let chunk = buf
            .get(..n)
            .ok_or(BlobError::Stream("source filled past the buffer"))?;
        data.extend_from_slice(chunk);
    }
    if body.fill(buf)? != 0 {
        return Err(BlobError::InvalidArgument(
            "body longer than its declared length",
        ));
    }
    Ok(data)
}

/// The half-open byte span `range` selects in an object of `len` bytes.
pub(super) fn span(range: ByteRange, len: usize) -> BlobResult<(usize, usize)> {
    match range {
        ByteRange::Full => Ok((0, len)),
        ByteRange::From { start, len: want } => {
            let start = usize::try_from(start).map_err(|_| BlobError::InvalidRange)?;
            if start >= len {
                return Err(BlobError::InvalidRange);
            }
            let want = usize::try_from(want).unwrap_or(usize::MAX);
            Ok((start, start.saturating_add(want).min(len)))
        }
    }
}
