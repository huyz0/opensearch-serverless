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
        // Each pass moves bytes or moves on to the next part, and the parts
        // are finite, so the loop ends.
        while let Some(room) = buf.get_mut(n..).filter(|room| !room.is_empty())
            && self.part <= self.parts.len()
        {
            let rest = self
                .current()
                .and_then(|bytes| bytes.get(self.at..))
                .unwrap_or_default();
            if rest.is_empty() {
                self.part = self.part.saturating_add(1);
                self.at = 0;
                continue;
            }
            let take = rest.len().min(room.len());
            let (Some(out), Some(from)) = (room.get_mut(..take), rest.get(..take)) else {
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

#[cfg(test)]
mod tests {
    use lucene_store::Input;
    use oss_blob::ChunkSource;

    use super::SegmentBlob;

    fn drain(blob: &mut SegmentBlob, buf_len: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = vec![0u8; buf_len];
        loop {
            let n = blob.fill(&mut buf).expect("fills");
            if n == 0 {
                return out;
            }
            out.extend_from_slice(buf.get(..n).expect("within"));
        }
    }

    fn blob() -> SegmentBlob {
        SegmentBlob::new(
            b"HDR".to_vec(),
            vec![
                Input::Owned(b"aaaa".to_vec()),
                Input::Owned(Vec::new()),
                Input::Owned(b"bb".to_vec()),
            ],
        )
    }

    #[test]
    fn the_header_then_each_part_streams_whatever_the_buffer() {
        let mut b = blob();
        assert_eq!(b.len(), 9);
        for size in [1, 2, 3, 4, 5, 9, 10, 64] {
            b.rewind().expect("rewinds");
            assert_eq!(drain(&mut b, size), b"HDRaaaabb", "buffer of {size}");
        }
    }

    #[test]
    fn a_full_buffer_ends_a_fill_and_the_next_fill_goes_on() {
        let mut b = blob();
        let mut buf = [0u8; 5];
        assert_eq!(b.fill(&mut buf).expect("fills"), 5);
        assert_eq!(&buf, b"HDRaa");
        assert_eq!(b.fill(&mut buf).expect("fills"), 4);
        assert_eq!(buf.get(..4), Some(&b"aabb"[..]));
        assert_eq!(b.fill(&mut buf).expect("fills"), 0);
    }

    #[test]
    fn an_empty_buffer_takes_nothing() {
        let mut b = blob();
        assert_eq!(b.fill(&mut []).expect("fills"), 0);
        assert_eq!(drain(&mut b, 64), b"HDRaaaabb");
    }
}
