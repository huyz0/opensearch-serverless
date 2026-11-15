//! A [`BlobStore`] whose bodies stream through buffers from the node's
//! [`BufferPool`].
//!
//! The store trait takes a caller buffer for each body; this wrapper checks
//! one out of the pool for the length of the call and returns it on every
//! path, success or error. When every buffer is in use the call fails with
//! [`BlobError::Busy`] before any request is sent or counted, so the caller
//! answers 429 or backs off instead of allocating (ADR 0004). No call here
//! touches the global allocator.

use oss_mem::BufferPool;

use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::store::{BlobStore, ByteRange, CreateOutcome, GetOutcome};

/// A store paired with the buffer pool its bodies stream through.
#[derive(Debug)]
pub struct PooledStore<S> {
    store: S,
    buffers: BufferPool,
}

impl<S: BlobStore> PooledStore<S> {
    /// Streams `store`'s bodies through buffers checked out of `buffers`.
    pub fn new(store: S, buffers: BufferPool) -> Self {
        Self { store, buffers }
    }

    /// The wrapped store, for the calls that carry no body.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The pool the bodies stream through.
    pub fn buffers(&self) -> &BufferPool {
        &self.buffers
    }

    /// [`BlobStore::get`] through a pooled buffer.
    pub fn get(
        &self,
        key: &str,
        range: ByteRange,
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<GetOutcome> {
        let mut buf = self.buffers.try_checkout().map_err(|_| BlobError::Busy)?;
        self.store.get(key, range, &mut buf, sink)
    }

    /// [`BlobStore::put`] through a pooled buffer.
    pub fn put(&self, key: &str, body: &mut dyn ChunkSource) -> BlobResult<()> {
        let mut buf = self.buffers.try_checkout().map_err(|_| BlobError::Busy)?;
        self.store.put(key, body, &mut buf)
    }

    /// [`BlobStore::create_if_absent`] through a pooled buffer.
    pub fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
    ) -> BlobResult<CreateOutcome> {
        let mut buf = self.buffers.try_checkout().map_err(|_| BlobError::Busy)?;
        self.store.create_if_absent(key, body, &mut buf)
    }
}

#[cfg(all(test, feature = "emulator"))]
mod tests {
    use oss_mem::{BudgetConfig, MemoryBudget, Pool};

    use super::*;
    use crate::chunk::SliceSource;
    use crate::emulator::Emulator;
    use crate::op::S3Op;

    fn pooled(buffers: usize) -> PooledStore<Emulator> {
        let config = BudgetConfig::new(1 << 20)
            .with_pool(Pool::Transport, 1 << 20)
            .expect("fits");
        let budget = MemoryBudget::new(&config);
        let pool = BufferPool::new(&budget, Pool::Transport, 4096, buffers).expect("fits");
        PooledStore::new(Emulator::new(1 << 20, 7), pool)
    }

    #[test]
    fn bodies_round_trip_and_buffers_come_back() {
        let s = pooled(1);
        s.put("k", &mut SliceSource::new(b"hello")).unwrap();
        assert_eq!(
            s.create_if_absent("k", &mut SliceSource::new(b"x"))
                .unwrap(),
            CreateOutcome::AlreadyExists
        );
        let mut got = [0u8; 5];
        let mut at = 0usize;
        let out = s
            .get("k", ByteRange::Full, &mut |c: &[u8]| {
                got[at..at + c.len()].copy_from_slice(c);
                at += c.len();
                Ok(())
            })
            .unwrap();
        assert_eq!((out.bytes, &got), (5, b"hello"));
        assert_eq!(s.buffers().available(), 1);
    }

    #[test]
    fn a_failed_call_still_returns_its_buffer() {
        let s = pooled(1);
        let err = s
            .get("missing", ByteRange::Full, &mut |_: &[u8]| Ok(()))
            .unwrap_err();
        assert_eq!(err, BlobError::NotFound);
        assert_eq!(s.buffers().available(), 1);
    }

    #[test]
    fn no_free_buffer_is_busy_and_sends_nothing() {
        let s = pooled(1);
        let held = s.buffers().try_checkout().expect("one free");
        assert_eq!(
            s.put("k", &mut SliceSource::new(b"x")),
            Err(BlobError::Busy)
        );
        assert_eq!(
            s.create_if_absent("k", &mut SliceSource::new(b"x")),
            Err(BlobError::Busy)
        );
        assert_eq!(
            s.get("k", ByteRange::Full, &mut |_: &[u8]| Ok(())),
            Err(BlobError::Busy)
        );
        for op in S3Op::ALL {
            assert_eq!(s.store().meter().count(op), 0, "{op:?}");
        }
        drop(held);
        assert!(s.put("k", &mut SliceSource::new(b"x")).is_ok());
    }
}
