//! Where a GET's bytes go and where a PUT's bytes come from.
//!
//! The Java shell passes `InputStream`s both ways (`readBlob` returns one,
//! `writeBlob` takes one with its size), as `MeteredBlobStore` shows
//! (`serverless/shell/src/main/java/org/opensearch/serverless/store/MeteredBlobStore.java`).
//! Here the store pushes a GET's bytes into a [`ChunkSink`] and pulls a PUT's
//! from a [`ChunkSource`], one caller-provided buffer at a time, so no
//! request holds more than one buffer of a body (ADR 0004).

use crate::error::{BlobError, BlobResult};

/// Receives a GET's body, one chunk at a time, in order.
pub trait ChunkSink {
    /// Takes the next chunk. A chunk is never empty and never larger than
    /// the buffer the caller gave the store. An error stops the GET.
    fn accept(&mut self, chunk: &[u8]) -> BlobResult<()>;
}

impl<F: FnMut(&[u8]) -> BlobResult<()>> ChunkSink for F {
    fn accept(&mut self, chunk: &[u8]) -> BlobResult<()> {
        self(chunk)
    }
}

/// Supplies a PUT's body, whose length is declared up front as S3's
/// `Content-Length` requires.
pub trait ChunkSource {
    /// The body's length in bytes.
    fn len(&self) -> u64;

    /// Whether the body is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fills the front of `buf` with the next bytes and returns how many;
    /// 0 means the body has ended.
    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize>;

    /// Starts the body again from its first byte, so a store can retry a
    /// failed attempt; the Java shell does this with
    /// `RepeatableBlobInputStream`. A source that cannot answers
    /// [`BlobError::Stream`], and the store then gives up after one attempt.
    fn rewind(&mut self) -> BlobResult<()>;
}

/// A body held in a slice the caller already owns.
#[derive(Debug)]
pub struct SliceSource<'a> {
    all: &'a [u8],
    rest: &'a [u8],
    len: u64,
}

impl<'a> SliceSource<'a> {
    /// A source that yields `bytes`.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            all: bytes,
            rest: bytes,
            len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        }
    }
}

impl ChunkSource for SliceSource<'_> {
    fn len(&self) -> u64 {
        self.len
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let n = self.rest.len().min(buf.len());
        let (head, tail) = self.rest.split_at(n);
        buf.get_mut(..n)
            .ok_or(BlobError::Stream("buffer shorter than its length"))?
            .copy_from_slice(head);
        self.rest = tail;
        Ok(n)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        self.rest = self.all;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_source_yields_its_bytes_in_buffer_sized_pieces() {
        let mut src = SliceSource::new(b"abcdefg");
        assert_eq!(src.len(), 7);
        assert!(!src.is_empty());
        let mut buf = [0u8; 3];
        assert_eq!(src.fill(&mut buf), Ok(3));
        assert_eq!(&buf, b"abc");
        assert_eq!(src.fill(&mut buf), Ok(3));
        assert_eq!(&buf, b"def");
        assert_eq!(src.fill(&mut buf), Ok(1));
        assert_eq!(buf[0], b'g');
        assert_eq!(src.fill(&mut buf), Ok(0));
        src.rewind().unwrap();
        assert_eq!(src.fill(&mut buf), Ok(3));
        assert_eq!(&buf, b"abc");
    }

    #[test]
    fn an_empty_slice_is_an_empty_source() {
        let src = SliceSource::new(b"");
        assert!(src.is_empty());
    }

    #[test]
    fn a_closure_is_a_sink() {
        let mut seen = 0usize;
        let mut sink = |chunk: &[u8]| {
            seen += chunk.len();
            Ok(())
        };
        sink.accept(b"xyz").unwrap();
        assert_eq!(seen, 3);
    }
}
