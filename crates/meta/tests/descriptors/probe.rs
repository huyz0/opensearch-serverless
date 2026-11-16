//! AC-1 and AC-2 of docs/specs/tombstone-sweep/SPEC.md: the conditional-delete probe.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use oss_blob::S3Op::{Delete, Get, GetRange, Put};
use oss_blob::{
    BlobError, BlobResult, BlobStore, ByteRange, ChunkSink, ChunkSource, CreateOutcome, Emulator,
    GetOutcome, ListPage, Meter, ObjectMeta, RegisterRead, SwapOutcome,
};
use oss_meta::{DeleteMode, PROBE_PREFIX};

use crate::{BASE, all, counted};

/// How a wrapped store misbehaves.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Quirk {
    /// Deletes a register whatever generation it holds, as a store that
    /// ignores `If-Match` on DELETE.
    IgnoreDeleteCondition,
    /// Fails the n-th request that reaches it (counting from 1).
    FailRequest(u32),
    /// Refuses every register create.
    RefuseCreates,
    /// Each LIST also visits a key under its prefix that does not exist, as
    /// a name deleted between its listing and its read.
    PhantomKey,
    /// From the n-th LIST on (counting from 1), answers an empty page that
    /// claims more follows.
    EmptyTruncatedFrom(u32),
    /// The first n register swaps and creates answer a conflict, as if
    /// another writer moved the register each time.
    ConflictSwaps(u32),
    /// None: the store behaves, and only a hook runs.
    Plain,
}

/// Another writer's step, run on the inner store just before an armed
/// request reaches it.
pub(crate) type Hook = Box<dyn Fn(&dyn BlobStore) + Send + Sync>;

/// A store that misbehaves as its quirk says, over an emulator.
pub(crate) struct Quirky {
    pub(crate) inner: Arc<dyn BlobStore>,
    quirk: Quirk,
    requests: AtomicU32,
    lists: AtomicU32,
    swaps: AtomicU32,
    hook: Option<Hook>,
    hook_at: AtomicU32,
}

impl Quirky {
    pub(crate) fn new(quirk: Quirk) -> Self {
        Self {
            inner: Arc::new(Emulator::new(16 << 20, 1)),
            quirk,
            requests: AtomicU32::new(0),
            lists: AtomicU32::new(0),
            swaps: AtomicU32::new(0),
            hook: None,
            hook_at: AtomicU32::new(0),
        }
    }

    /// A store that behaves, but runs `hook` once armed.
    pub(crate) fn hooked(hook: impl Fn(&dyn BlobStore) + Send + Sync + 'static) -> Self {
        Self {
            hook: Some(Box::new(hook)),
            ..Self::new(Quirk::Plain)
        }
    }

    /// Runs the hook before the `n`-th request from now (counting from 1).
    pub(crate) fn arm(&self, n: u32) {
        let at = self.requests.load(Ordering::SeqCst) + n;
        self.hook_at.store(at, Ordering::SeqCst);
    }

    /// Whether this register swap or create is one the quirk refuses.
    fn conflicts(&self) -> bool {
        let n = self.swaps.fetch_add(1, Ordering::SeqCst) + 1;
        matches!(self.quirk, Quirk::ConflictSwaps(first) if n <= first)
    }

    fn request(&self) -> BlobResult<()> {
        let n = self.requests.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(hook) = &self.hook
            && self.hook_at.load(Ordering::SeqCst) == n
        {
            hook(&*self.inner);
        }
        if self.quirk == Quirk::FailRequest(n) {
            return Err(BlobError::Protocol("planted failure"));
        }
        Ok(())
    }
}

impl BlobStore for Quirky {
    fn meter(&self) -> &Meter {
        self.inner.meter()
    }
    fn validate_key(&self, key: &str) -> BlobResult<()> {
        self.inner.validate_key(key)
    }
    fn get(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<GetOutcome> {
        self.request()?;
        self.inner.get(key, range, buf, sink)
    }
    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
        self.request()?;
        self.inner.put(key, body, buf)
    }
    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        self.request()?;
        self.inner.create_if_absent(key, body, buf)
    }
    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        self.request()?;
        self.inner.head(key)
    }
    fn delete(&self, key: &str) -> BlobResult<()> {
        self.request()?;
        self.inner.delete(key)
    }
    // LIST: the wrapper passes the call through.
    fn list_prefix(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        max_keys: usize,
        visit: &mut dyn FnMut(&str, u64),
    ) -> BlobResult<ListPage> {
        self.request()?;
        let n = self.lists.fetch_add(1, Ordering::SeqCst) + 1;
        if matches!(self.quirk, Quirk::EmptyTruncatedFrom(from) if n >= from) {
            return Ok(ListPage {
                keys: 0,
                truncated: true,
            });
        }
        if self.quirk == Quirk::PhantomKey {
            visit(&format!("{prefix}~gone"), 0);
        }
        // LIST: passed through.
        self.inner.list_prefix(prefix, start_after, max_keys, visit)
    }
    fn read_register(&self, key: &str, buf: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
        self.request()?;
        self.inner.read_register(key, buf)
    }
    fn swap_register(&self, key: &str, expected: u64, value: &[u8]) -> BlobResult<SwapOutcome> {
        self.request()?;
        if self.conflicts() {
            return Ok(SwapOutcome::Conflict(None));
        }
        self.inner.swap_register(key, expected, value)
    }
    fn create_register(&self, key: &str, value: &[u8]) -> BlobResult<SwapOutcome> {
        self.request()?;
        if self.quirk == Quirk::RefuseCreates || self.conflicts() {
            return Ok(SwapOutcome::Conflict(None));
        }
        self.inner.create_register(key, value)
    }
    fn delete_register(&self, key: &str, expected: u64) -> BlobResult<bool> {
        self.request()?;
        if self.quirk == Quirk::IgnoreDeleteCondition {
            let existed = self.inner.head(key)?.is_some();
            self.inner.delete(key)?;
            return Ok(existed);
        }
        self.inner.delete_register(key, expected)
    }
}

fn probe_keys(store: &dyn BlobStore) -> usize {
    store
        // LIST: a test checking the probe left nothing.
        .list_prefix(&format!("{BASE}{PROBE_PREFIX}"), None, 10, &mut |_, _| {})
        .unwrap()
        .keys
}

#[test]
fn every_backend_honours_a_conditional_delete() {
    for b in all() {
        let (mode, n) = counted(b.meter(), || DeleteMode::probe(&*b.store, BASE));
        assert_eq!(mode, DeleteMode::Conditional, "{:?}", b.kind);
        assert_eq!(
            n,
            b.budget(
                &[(Get, 3), (Put, 2), (Delete, 3)],
                &[(Get, 3), (GetRange, 2), (Put, 2), (Delete, 2)]
            ),
            "{:?}",
            b.kind
        );
        assert_eq!(probe_keys(&*b.store), 0, "{:?} cleaned up", b.kind);
    }
}

#[test]
fn a_store_that_ignores_the_condition_gets_tombstones() {
    let store = Quirky::new(Quirk::IgnoreDeleteCondition);
    assert_eq!(DeleteMode::probe(&store, BASE), DeleteMode::Tombstone);
    assert_eq!(probe_keys(&*store.inner), 0);
}

#[test]
fn any_failure_or_refusal_gets_tombstones() {
    // Each of the probe's seven steps failing in turn, then a refused create.
    for n in 1..=7 {
        let store = Quirky::new(Quirk::FailRequest(n));
        assert_eq!(
            DeleteMode::probe(&store, BASE),
            DeleteMode::Tombstone,
            "request {n}"
        );
        assert_eq!(probe_keys(&*store.inner), 0, "request {n} cleaned up");
    }
    // The cleanup delete failing changes no answer.
    let store = Quirky::new(Quirk::FailRequest(8));
    assert_eq!(DeleteMode::probe(&store, BASE), DeleteMode::Conditional);
    let store = Quirky::new(Quirk::RefuseCreates);
    assert_eq!(DeleteMode::probe(&store, BASE), DeleteMode::Tombstone);
}

#[test]
fn each_probe_uses_its_own_key() {
    let store = Quirky::new(Quirk::FailRequest(8));
    // The failed cleanup leaves the first probe's key; a second probe still
    // runs on a key of its own and answers.
    assert_eq!(DeleteMode::probe(&store, BASE), DeleteMode::Conditional);
    assert_eq!(DeleteMode::probe(&store, BASE), DeleteMode::Conditional);
}
