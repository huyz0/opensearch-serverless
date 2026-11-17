//! A segment blob as a PUT's body: its header, then the segment's files in
//! blob order, each read from the directory as the store asks for bytes.

use lucene_store::Input;
use oss_blob::{BlobError, BlobResult, ChunkSource};

/// A segment blob laid out and ready to stream: the header and the opened
/// files, nothing copied.
pub(super) struct SegmentBlob {
    header: Vec<u8>,
    parts: Vec<Input>,
    len: u64,
    /// Which part is being read: 0 is the header, then the files.
    part: usize,
    /// How far into that part.
    at: usize,
}

impl SegmentBlob {
    /// A blob of `header` followed by `parts` in order.
    pub(super) fn new(header: Vec<u8>, parts: Vec<Input>) -> Self {
        let len = parts
            .iter()
            .fold(u64::try_from(header.len()).unwrap_or(u64::MAX), |sum, p| {
                sum.saturating_add(u64::try_from(p.len()).unwrap_or(u64::MAX))
            });
        Self {
            header,
            parts,
            len,
            part: 0,
            at: 0,
        }
    }

    fn current(&self) -> Option<&[u8]> {
        match self.part {
            0 => Some(self.header.as_slice()),
            n => self.parts.get(n.saturating_sub(1)).map(|p| &**p),
        }
    }
}

impl ChunkSource for SegmentBlob {
    fn len(&self) -> u64 {
        self.len
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let mut n = 0usize;
        while n < buf.len() {
            let Some(bytes) = self.current() else { break };
            let rest = bytes.get(self.at..).unwrap_or_default();
            if rest.is_empty() {
                self.part = self.part.saturating_add(1);
                self.at = 0;
                continue;
            }
            let take = rest.len().min(buf.len().saturating_sub(n));
            let (Some(out), Some(from)) =
                (buf.get_mut(n..n.saturating_add(take)), rest.get(..take))
            else {
                return Err(BlobError::Stream("a segment blob's part is out of range"));
            };
            out.copy_from_slice(from);
            n = n.saturating_add(take);
            self.at = self.at.saturating_add(take);
        }
        Ok(n)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        self.part = 0;
        self.at = 0;
        Ok(())
    }
}
