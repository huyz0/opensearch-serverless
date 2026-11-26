//! A store an export writes a stock repository into.
//!
//! It writes only under one prefix, and only ways that cannot destroy what
//! the target already holds: every blob is created with a conditional
//! create (so a blob that exists is never overwritten), and `index.latest`
//! is the one key that may be put over. Every delete, listing and register
//! call is answered [`BlobError::Denied`] before it reaches the wrapped
//! store, so a request that names a target can never remove from it or pay
//! for a LIST. Reads and heads go through the wrapped store, metered by it.

use std::sync::Arc;

use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::meter::Meter;
use crate::register::{RegisterRead, SwapOutcome};
use crate::store::{BlobStore, ByteRange, CreateOutcome, GetOutcome, ListPage, ObjectMeta};

/// The key under a repository that points at its newest `index-N`.
const LATEST: &str = "index.latest";

/// A write-under-a-prefix view of a store.
pub struct ExportStore {
    inner: Arc<dyn BlobStore>,
    prefix: String,
}

impl ExportStore {
    /// A view of `inner` that reads and creates only under `prefix` (empty
    /// for a store whose root is the repository).
    #[must_use]
    pub fn new(inner: Arc<dyn BlobStore>, prefix: &str) -> Self {
        Self {
            inner,
            prefix: prefix.to_owned(), // ALLOC: one prefix a store.
        }
    }

    /// Whether `key` is under the prefix and climbs nowhere.
    fn allows(&self, key: &str) -> BlobResult<()> {
        let rest = key.strip_prefix(self.prefix.as_str()).ok_or(WHY)?;
        let clean = !key.starts_with('/')
            && key
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..");
        if rest.is_empty() || !clean {
            return Err(WHY);
        }
        Ok(())
    }
}

const WHY: BlobError = BlobError::Denied;

impl BlobStore for ExportStore {
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
        self.allows(key)?;
        self.inner.get(key, range, buf, sink)
    }

    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
        self.allows(key)?;
        if key.strip_prefix(self.prefix.as_str()) != Some(LATEST) {
            return Err(WHY);
        }
        self.inner.put(key, body, buf)
    }

    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        self.allows(key)?;
        self.inner.create_if_absent(key, body, buf)
    }

    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        self.allows(key)?;
        self.inner.head(key)
    }

    fn delete(&self, _key: &str) -> BlobResult<()> {
        Err(WHY)
    }

    fn delete_many(&self, _keys: &[&str]) -> BlobResult<()> {
        Err(WHY)
    }

    // LIST: refused, not issued: an export writes derived keys.
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

    fn read_register(&self, _key: &str, _buf: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
        Err(WHY)
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
