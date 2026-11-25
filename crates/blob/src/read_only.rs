//! A store that only reads: the source of a restore from a repository
//! another system wrote.
//!
//! Every write, delete and listing is answered [`BlobError::Denied`] before
//! it reaches the wrapped store, so a request that names a source can never
//! change it, and a restore that would LIST fails instead of paying for it.
//! Reads go through the wrapped store and are metered by it.

use std::sync::Arc;

use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::meter::Meter;
use crate::register::{RegisterRead, SwapOutcome};
use crate::store::{BlobStore, ByteRange, CreateOutcome, GetOutcome, ListPage, ObjectMeta};

/// A read-only view of a store.
pub struct ReadOnlyStore {
    inner: Arc<dyn BlobStore>,
}

impl ReadOnlyStore {
    /// A view of `inner` that answers only reads.
    #[must_use]
    pub fn new(inner: Arc<dyn BlobStore>) -> Self {
        Self { inner }
    }
}

const WHY: BlobError = BlobError::Denied;

impl BlobStore for ReadOnlyStore {
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

    fn put(&self, _key: &str, _body: &mut dyn ChunkSource, _buf: &mut [u8]) -> BlobResult<()> {
        Err(WHY)
    }

    fn create_if_absent(
        &self,
        _key: &str,
        _body: &mut dyn ChunkSource,
        _buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        Err(WHY)
    }

    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        self.inner.head(key)
    }

    fn delete(&self, _key: &str) -> BlobResult<()> {
        Err(WHY)
    }

    fn delete_many(&self, _keys: &[&str]) -> BlobResult<()> {
        Err(WHY)
    }

    // LIST: refused, not issued: a source is read by derived keys.
    fn list_prefix(
        &self,
        _prefix: &str,
        _start_after: Option<&str>,
        _max_keys: usize,
        _visit: &mut dyn FnMut(&str, u64),
    ) -> BlobResult<ListPage> {
        Err(WHY)
    }

    // LIST: refused, not issued.
    fn list_children(
        &self,
        _prefix: &str,
        _start_after: Option<&str>,
        _max_children: usize,
        _visit: &mut dyn FnMut(&str),
    ) -> BlobResult<ListPage> {
        Err(WHY)
    }

    fn read_register(&self, key: &str, buf: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
        self.inner.read_register(key, buf)
    }

    fn swap_register(&self, _key: &str, _expected: u64, _value: &[u8]) -> BlobResult<SwapOutcome> {
        Err(WHY)
    }

    fn create_register(&self, _key: &str, _value: &[u8]) -> BlobResult<SwapOutcome> {
        Err(WHY)
    }

    fn delete_register(&self, _key: &str, _expected: u64) -> BlobResult<bool> {
        Err(WHY)
    }

    fn validate_key(&self, key: &str) -> BlobResult<()> {
        self.inner.validate_key(key)
    }
}
