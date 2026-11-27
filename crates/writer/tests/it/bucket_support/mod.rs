//! A store whose register swaps lose a race first, for the bucket tests.

// ARITH: test sizes are small; an overflow panics the test.
#![allow(dead_code, clippy::arithmetic_side_effects)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_blob::register::RegisterRead;
use oss_blob::{
    BlobResult, BlobStore, ByteRange, ChunkSink, ChunkSource, CreateOutcome, Emulator, GetOutcome,
    Meter, ObjectMeta, SwapOutcome,
};

/// A store whose register swaps lose a race first: another writer moves the
/// register just before the swap is judged.
pub struct Racing {
    pub inner: Arc<Emulator>,
    pub races: AtomicUsize,
}

impl BlobStore for Racing {
    fn meter(&self) -> &Meter {
        self.inner.meter()
    }
    fn get(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<GetOutcome> {
        self.inner.get(key, range, buf, sink)
    }
    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
        self.inner.put(key, body, buf)
    }
    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        self.inner.create_if_absent(key, body, buf)
    }
    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        self.inner.head(key)
    }
    fn delete(&self, key: &str) -> BlobResult<()> {
        self.inner.delete(key)
    }
    // LIST: a pass-through of the trait method; no test here lists.
    fn list_prefix(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        max_keys: usize,
        visit: &mut dyn FnMut(&str, u64),
    ) -> BlobResult<oss_blob::ListPage> {
        // LIST: the same pass-through.
        self.inner.list_prefix(prefix, start_after, max_keys, visit)
    }
    fn read_register(&self, key: &str, buf: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
        self.inner.read_register(key, buf)
    }
    fn swap_register(&self, key: &str, expected: u64, value: &[u8]) -> BlobResult<SwapOutcome> {
        if self.races.load(Ordering::SeqCst) > 0 {
            self.races.fetch_sub(1, Ordering::SeqCst);
            let mut buf = vec![0u8; 1 << 20];
            let read = self.inner.read_register(key, &mut buf)?.expect("exists");
            let same = buf[..read.len].to_vec();
            self.inner.swap_register(key, read.generation, &same)?;
        }
        self.inner.swap_register(key, expected, value)
    }
    fn create_register(&self, key: &str, value: &[u8]) -> BlobResult<SwapOutcome> {
        self.inner.create_register(key, value)
    }
    fn delete_register(&self, key: &str, expected: u64) -> BlobResult<bool> {
        self.inner.delete_register(key, expected)
    }
}
