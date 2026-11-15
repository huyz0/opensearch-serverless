//! A PUT body as the HTTP client reads it.

use std::io::Read;

use crate::chunk::ChunkSource;
use crate::error::{BlobError, BlobResult};

/// A PUT body as `ureq` reads it: filled from the source into the caller's
/// buffer, and checked against its declared length before its last byte
/// goes out, so a body that is too long or too short never completes and
/// the store commits nothing.
pub(super) struct Body<'a> {
    pub(super) source: &'a mut dyn ChunkSource,
    pub(super) buf: &'a mut [u8],
    pub(super) filled: usize,
    pub(super) pos: usize,
    pub(super) sent: u64,
    pub(super) len: u64,
    /// The source's failure, kept so the caller gets it rather than the
    /// I/O error `ureq` wraps it in.
    pub(super) failure: Option<BlobError>,
}

impl Body<'_> {
    fn refill(&mut self) -> BlobResult<()> {
        let n = self.source.fill(self.buf)?;
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        let sent = self.sent.saturating_add(n64);
        if n == 0 {
            return Err(BlobError::InvalidArgument(
                "body shorter than its declared length",
            ));
        }
        if n > self.buf.len() {
            return Err(BlobError::Stream("source filled past the buffer"));
        }
        if sent > self.len || (sent == self.len && self.source.fill(&mut [0u8; 1])? != 0) {
            return Err(BlobError::InvalidArgument(
                "body longer than its declared length",
            ));
        }
        self.sent = sent;
        self.filled = n;
        self.pos = 0;
        Ok(())
    }
}

impl Read for Body<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.filled {
            if self.sent >= self.len {
                return Ok(0);
            }
            if let Err(err) = self.refill() {
                self.failure = Some(err);
                return Err(std::io::Error::other("PUT body failed"));
            }
        }
        let ready = self.buf.get(self.pos..self.filled).unwrap_or_default();
        let n = ready.len().min(out.len());
        if let (Some(dst), Some(src)) = (out.get_mut(..n), ready.get(..n)) {
            dst.copy_from_slice(src);
        }
        self.pos = self.pos.saturating_add(n);
        Ok(n)
    }
}
