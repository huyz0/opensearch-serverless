//! Round-trip depth: how many object-store round trips a caller waits
//! through in sequence, not how many requests it sends.
//!
//! Ported from the shell's `RoundTripDepthBlobStore`
//! (`serverless/testkit/src/test/java/org/opensearch/serverless/testkit/RoundTripDepthBlobStore.java@3920dc36`).
//! A call is one round deeper than the deepest call already answered when
//! it started: calls sent together, before any answers, share a round; a
//! call sent after reading another's answer is the next round. Each call
//! can be held for a fixed delay, so calls sent together overlap even on a
//! store that answers in microseconds. Every call here is one round: one
//! request on the emulator. On S3 a register swap or conditional delete
//! from a generation this process did not just read is a read and then a
//! write, two requests in sequence, which this counts as one round.
//!
//! The measure is an upper bound: an unrelated call that starts after the
//! measured path's last answer counts one round deeper, so a measurement
//! runs with nothing else talking to the store.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::BlobResult;
use crate::meter::Meter;
use crate::register::{RegisterRead, SwapOutcome};
use crate::store::{BlobStore, ByteRange, CreateOutcome, GetOutcome, ListPage, ObjectMeta};

/// Wraps a store and measures the round-trip depth of the calls made
/// through it, logging each call's kind and key.
#[derive(Debug)]
pub struct Rounds<S> {
    inner: S,
    delay_nanos: AtomicU64,
    finished: AtomicUsize,
    deepest: AtomicUsize,
    log: Mutex<Vec<(&'static str, String)>>,
}

impl<S: BlobStore> Rounds<S> {
    /// Wraps `inner`, holding no call until a measurement starts.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            delay_nanos: AtomicU64::new(0),
            finished: AtomicUsize::new(0),
            deepest: AtomicUsize::new(0),
            // ALLOC: test support only; one entry a call made during a
            // measurement.
            log: Mutex::new(Vec::new()),
        }
    }

    /// The wrapped store.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Starts a new measurement: the next call is round one, the log is
    /// empty, and each call from now on is held `delay`; zero runs at full
    /// speed.
    pub fn reset(&self, delay: Duration) {
        let nanos = u64::try_from(delay.as_nanos()).unwrap_or(u64::MAX);
        self.delay_nanos.store(nanos, Ordering::SeqCst);
        self.finished.store(0, Ordering::SeqCst);
        self.deepest.store(0, Ordering::SeqCst);
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// The deepest round a call reached since the last reset: sequential
    /// round trips, zero when nothing was called.
    pub fn depth(&self) -> usize {
        self.deepest.load(Ordering::SeqCst)
    }

    /// The calls made since the last reset, in the order they started: each
    /// call's kind (the trait method) and key, a ranged read's key followed
    /// by `[start+len]`.
    pub fn calls(&self) -> Vec<(&'static str, String)> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The calls since the last reset, one a line, for a failed gate to
    /// name what a path touched.
    pub fn detail(&self) -> String {
        let calls = self.calls();
        // ALLOC: test support only; one line a call, for a failed gate.
        let mut out = format!("{} calls, depth {}:", calls.len(), self.depth());
        for (kind, key) in calls {
            out.push_str("\n  ");
            out.push_str(kind);
            out.push(' ');
            out.push_str(&key);
        }
        out
    }

    /// Runs one call as one round.
    fn round<T>(&self, kind: &'static str, key: &str, call: impl FnOnce(&S) -> T) -> T {
        let round = self.finished.load(Ordering::SeqCst).saturating_add(1);
        self.deepest.fetch_max(round, Ordering::SeqCst);
        // ALLOC: test support only; the key of one call.
        let entry = (kind, key.to_owned());
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry);
        // A zero delay sleeps not at all.
        std::thread::sleep(Duration::from_nanos(
            self.delay_nanos.load(Ordering::SeqCst),
        ));
        let answer = call(&self.inner);
        self.finished.fetch_max(round, Ordering::SeqCst);
        answer
    }
}

impl<S: BlobStore> BlobStore for Rounds<S> {
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
        match range {
            ByteRange::Full => self.round("get", key, |s| s.get(key, range, buf, sink)),
            ByteRange::From { start, len } => {
                // ALLOC: test support only; one logged key a ranged read.
                let at = format!("{key} [{start}+{len}]");
                self.round("get", &at, |s| s.get(key, range, buf, sink))
            }
        }
    }

    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
        self.round("put", key, |s| s.put(key, body, buf))
    }

    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        self.round("create_if_absent", key, |s| {
            s.create_if_absent(key, body, buf)
        })
    }

    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        self.round("head", key, |s| s.head(key))
    }

    fn delete(&self, key: &str) -> BlobResult<()> {
        self.round("delete", key, |s| s.delete(key))
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
        self.round("list_prefix", prefix, |s| {
            // LIST: the caller's one page, passed through.
            s.list_prefix(prefix, start_after, max_keys, visit)
        })
    }

    fn read_register(&self, key: &str, buf: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
        self.round("read_register", key, |s| s.read_register(key, buf))
    }

    fn swap_register(&self, key: &str, expected: u64, value: &[u8]) -> BlobResult<SwapOutcome> {
        self.round("swap_register", key, |s| {
            s.swap_register(key, expected, value)
        })
    }

    fn create_register(&self, key: &str, value: &[u8]) -> BlobResult<SwapOutcome> {
        self.round("create_register", key, |s| s.create_register(key, value))
    }

    fn delete_register(&self, key: &str, expected: u64) -> BlobResult<bool> {
        self.round("delete_register", key, |s| s.delete_register(key, expected))
    }

    fn validate_key(&self, key: &str) -> BlobResult<()> {
        self.inner.validate_key(key)
    }
}
