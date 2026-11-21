//! One store behind a handle every role of a node can hold.

use std::sync::Arc;

use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::BlobResult;
use crate::meter::Meter;
use crate::register::{RegisterRead, SwapOutcome};
use crate::store::{BlobStore, ByteRange, CreateOutcome, GetOutcome, ListPage, ObjectMeta};

/// A [`BlobStore`] that forwards every call to a store shared with others:
/// the reader, writer and janitor of one node reach one store, one meter.
#[derive(Clone)]
pub struct Shared(Arc<dyn BlobStore>);

impl Shared {
    /// A handle on `store`.
    pub fn new(store: Arc<dyn BlobStore>) -> Self {
        Self(store)
    }
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Shared")
    }
}

impl BlobStore for Shared {
    fn meter(&self) -> &Meter {
        self.0.meter()
    }

    fn get(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<GetOutcome> {
        self.0.get(key, range, buf, sink)
    }

    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
        self.0.put(key, body, buf)
    }

    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        self.0.create_if_absent(key, body, buf)
    }

    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        self.0.head(key)
    }

    fn delete(&self, key: &str) -> BlobResult<()> {
        self.0.delete(key)
    }

    fn delete_many(&self, keys: &[&str]) -> BlobResult<()> {
        self.0.delete_many(keys)
    }

    // LIST: passes the caller's one bounded page through; the caller carries
    // its own justification.
    fn list_prefix(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        max_keys: usize,
        visit: &mut dyn FnMut(&str, u64),
    ) -> BlobResult<ListPage> {
        // LIST: the caller's one page, passed through.
        self.0.list_prefix(prefix, start_after, max_keys, visit)
    }

    fn read_register(&self, key: &str, buf: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
        self.0.read_register(key, buf)
    }

    fn swap_register(&self, key: &str, expected: u64, value: &[u8]) -> BlobResult<SwapOutcome> {
        self.0.swap_register(key, expected, value)
    }

    fn create_register(&self, key: &str, value: &[u8]) -> BlobResult<SwapOutcome> {
        self.0.create_register(key, value)
    }

    fn delete_register(&self, key: &str, expected: u64) -> BlobResult<bool> {
        self.0.delete_register(key, expected)
    }

    fn validate_key(&self, key: &str) -> BlobResult<()> {
        self.0.validate_key(key)
    }
}
