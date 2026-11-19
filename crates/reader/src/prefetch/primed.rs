//! The bytes an open prefetch fetched, served to the engine's reads.

use std::collections::BTreeMap;

use oss_blob::{BlobError, BlobStore, ByteRange, PooledStore};

use super::{MAX_CONCURRENT, Range};

/// Where a file's bytes are: buffer, start inside it, length.
type Slot = (usize, usize, usize);

/// Files fetched ahead of the engine's open.
#[derive(Debug, Default)]
pub struct Primed {
    buffers: Vec<Vec<u8>>,
    files: BTreeMap<String, Slot>,
}

impl Primed {
    /// Fetches `ranges`, up to 16 at once. A range the store
    /// refuses is left out: the engine reads its files itself.
    pub fn fetch<S: BlobStore>(store: &PooledStore<S>, ranges: &[Range]) -> Self {
        let mut primed = Self::default();
        for batch in ranges.chunks(MAX_CONCURRENT) {
            let fetched: Vec<Option<Vec<u8>>> = std::thread::scope(|scope| {
                let handles: Vec<_> = batch
                    .iter()
                    .map(|range| scope.spawn(move || read(store, range)))
                    // ALLOC: one handle a range, at most MAX_CONCURRENT.
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().ok().flatten())
                    // ALLOC: one result a range, at most MAX_CONCURRENT.
                    .collect()
            });
            for (range, bytes) in batch.iter().zip(fetched) {
                if let Some(bytes) = bytes {
                    primed.add(range, bytes);
                }
            }
        }
        primed
    }

    fn add(&mut self, range: &Range, bytes: Vec<u8>) {
        let buffer = self.buffers.len();
        for m in &range.members {
            let (Ok(at), Ok(len)) = (usize::try_from(m.at), usize::try_from(m.len)) else {
                continue;
            };
            if at.checked_add(len).is_some_and(|end| end <= bytes.len()) {
                // ALLOC: one name a primed file.
                self.files.insert(m.name.clone(), (buffer, at, len));
            }
        }
        self.buffers.push(bytes);
    }

    /// Copies `name`'s bytes from `offset` into `out`, and how many: `None`
    /// when `name` was not fetched.
    pub fn read(&self, name: &str, offset: u64, out: &mut [u8]) -> Option<usize> {
        let &(buffer, at, len) = self.files.get(name)?;
        let offset = usize::try_from(offset).ok()?;
        let n = out.len().min(len.saturating_sub(offset));
        let from = at.checked_add(offset)?;
        let src = self.buffers.get(buffer)?.get(from..from.checked_add(n)?)?;
        out.get_mut(..n)?.copy_from_slice(src);
        Some(n)
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }

    /// Releases every byte.
    pub fn clear(&mut self) {
        // ALLOC: empty, holds nothing.
        self.buffers = Vec::new();
        self.files.clear();
    }
}

fn read<S: BlobStore>(store: &PooledStore<S>, range: &Range) -> Option<Vec<u8>> {
    let want = usize::try_from(range.len).ok()?;
    // ALLOC: one range, at most `OpenPrefetch::max_bytes` plus the gaps it spans.
    let mut bytes = Vec::with_capacity(want);
    store
        .get(
            &range.key,
            ByteRange::From {
                start: range.start,
                len: range.len,
            },
            &mut |chunk: &[u8]| {
                if bytes.len().saturating_add(chunk.len()) > want {
                    return Err(BlobError::Stream("store sent more than was asked"));
                }
                bytes.extend_from_slice(chunk);
                Ok(())
            },
        )
        .ok()?;
    (bytes.len() == want).then_some(bytes)
}
