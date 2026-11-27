//! A store whose every request is charged to an index.

use std::sync::Arc;

use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::BlobResult;
use crate::meter::Meter;
use crate::register::{RegisterRead, SwapOutcome};
use crate::store::{BlobStore, ByteRange, CreateOutcome, GetOutcome, ListPage, ObjectMeta};
use crate::usage::{Classifier, Ledger, LedgerTap};

/// Wraps a store so its meter's attempts are charged in a [`Ledger`]: to the
/// index a call's key names, else to what the thread works for
/// ([`Ledger::split`]), else to the node. Wrap the store that owns the
/// meter, not a store that wraps it, or the retries it makes are not seen.
pub struct UsageStore<S> {
    inner: S,
    ledger: Arc<Ledger>,
    classifier: Arc<dyn Classifier>,
}

impl<S: BlobStore> UsageStore<S> {
    /// `inner` charging `ledger`, finding a key's index with `classifier`.
    /// A store already tapped by another ledger is left charging that one.
    pub fn new(inner: S, ledger: Arc<Ledger>, classifier: Arc<dyn Classifier>) -> Self {
        inner
            .meter()
            .set_tap(Arc::new(LedgerTap(Arc::clone(&ledger))));
        Self {
            inner,
            ledger,
            classifier,
        }
    }

    /// The ledger charged.
    pub fn ledger(&self) -> &Arc<Ledger> {
        &self.ledger
    }
}

impl<S> std::fmt::Debug for UsageStore<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UsageStore")
    }
}

impl<S: BlobStore> BlobStore for UsageStore<S> {
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
        let _owner = self.ledger.enter(self.classifier.index_of(key));
        self.inner.get(key, range, buf, sink)
    }

    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
        let _owner = self.ledger.enter(self.classifier.index_of(key));
        self.inner.put(key, body, buf)
    }

    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        let _owner = self.ledger.enter(self.classifier.index_of(key));
        self.inner.create_if_absent(key, body, buf)
    }

    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        let _owner = self.ledger.enter(self.classifier.index_of(key));
        self.inner.head(key)
    }

    fn age_millis(&self, key: &str) -> BlobResult<Option<u64>> {
        let _owner = self.ledger.enter(self.classifier.index_of(key));
        self.inner.age_millis(key)
    }

    fn delete(&self, key: &str) -> BlobResult<()> {
        let _owner = self.ledger.enter(self.classifier.index_of(key));
        self.inner.delete(key)
    }

    fn delete_many(&self, keys: &[&str]) -> BlobResult<()> {
        // A batch is one request: charged to the index all its keys name,
        // else to the node.
        let mut owners = keys.iter().map(|key| self.classifier.index_of(key));
        let common = owners
            .next()
            .flatten()
            .filter(|first| owners.all(|owner| owner == Some(first)));
        let _owner = self.ledger.enter(common);
        self.inner.delete_many(keys)
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
        let _owner = self.ledger.enter(self.classifier.index_of(prefix));
        // LIST: the caller's one page, passed through.
        self.inner.list_prefix(prefix, start_after, max_keys, visit)
    }

    fn read_register(&self, key: &str, buf: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
        let _owner = self.ledger.enter(self.classifier.index_of(key));
        self.inner.read_register(key, buf)
    }

    fn swap_register(&self, key: &str, expected: u64, value: &[u8]) -> BlobResult<SwapOutcome> {
        let _owner = self.ledger.enter(self.classifier.index_of(key));
        self.inner.swap_register(key, expected, value)
    }

    fn create_register(&self, key: &str, value: &[u8]) -> BlobResult<SwapOutcome> {
        let _owner = self.ledger.enter(self.classifier.index_of(key));
        self.inner.create_register(key, value)
    }

    fn delete_register(&self, key: &str, expected: u64) -> BlobResult<bool> {
        let _owner = self.ledger.enter(self.classifier.index_of(key));
        self.inner.delete_register(key, expected)
    }

    fn validate_key(&self, key: &str) -> BlobResult<()> {
        self.inner.validate_key(key)
    }
}
