//! A range of one blob as a PUT's body.

use oss_blob::{BlobError, BlobResult, BlobStore, ByteRange, ChunkSource};

/// A range of one blob as a PUT's body: each fill is one ranged read of at
/// most the scratch buffer.
pub(crate) struct RangedSource<'a> {
    pub(crate) store: &'a dyn BlobStore,
    pub(crate) key: &'a str,
    pub(crate) start: u64,
    pub(crate) length: u64,
    pub(crate) done: u64,
    pub(crate) scratch: &'a mut [u8],
}

impl ChunkSource for RangedSource<'_> {
    fn len(&self) -> u64 {
        self.length
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let left = self.length.saturating_sub(self.done);
        let want = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
        if want == 0 {
            return Ok(0);
        }
        let range = ByteRange::From {
            start: self.start.saturating_add(self.done),
            len: u64::try_from(want).unwrap_or(u64::MAX),
        };
        let mut filled = 0usize;
        let mut sink = |chunk: &[u8]| -> BlobResult<()> {
            let end = filled.saturating_add(chunk.len());
            buf.get_mut(filled..end)
                .ok_or(BlobError::Stream("a read returned more than asked"))?
                .copy_from_slice(chunk);
            filled = end;
            Ok(())
        };
        self.store
            .get(self.key, range, &mut *self.scratch, &mut sink)?;
        if filled != want {
            return Err(BlobError::Stream(
                "a captured file is shorter than recorded",
            ));
        }
        self.done = self
            .done
            .saturating_add(u64::try_from(want).unwrap_or(u64::MAX));
        Ok(want)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        self.done = 0;
        Ok(())
    }
}
