//! An in-process S3 emulator, so every test runs without Docker or a
//! network (invariant I-7 of AGENTS.md, ADR 0005).
//!
//! It stands in for the Java shell's test-kit stores behind
//! `MeteredBlobStore` (`serverless/shell/src/main/java/org/opensearch/serverless/store/MeteredBlobStore.java@bf347adb`,
//! which counts what they serve): it keeps objects in memory up to a fixed
//! byte capacity, answers with S3's semantics (atomic replace, conditional
//! create decided at commit, 416 past the end, delete of a missing key
//! succeeds, LIST pages of at most 1,000 keys in byte order), and records
//! every request it serves, including the ones it fails on purpose, in its
//! [`Meter`]. Latency, failures and throttling are injected per request
//! kind from a seed ([`faults`]).

pub mod faults;

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::meter::Meter;
use crate::op::S3Op;
use crate::store::{
    BlobStore, ByteRange, CreateOutcome, GetOutcome, ListPage, ObjectMeta, check_buf, check_key,
    check_list,
};
use faults::{Decision, Faults, Latency, OpControl, SplitMix64};

/// The stored objects and the bytes they, and PUTs in flight, hold.
#[derive(Debug, Default)]
struct Objects {
    map: BTreeMap<String, Arc<Vec<u8>>>,
    stored: u64,
    reserved: u64,
}

/// An in-memory object store with S3's semantics and injectable faults.
#[derive(Debug)]
pub struct Emulator {
    capacity: u64,
    objects: Mutex<Objects>,
    control: Mutex<[OpControl; S3Op::ALL.len()]>,
    meter: Meter,
}

/// Locks `m`, recovering the data if a panicking thread poisoned it: the
/// emulator's state is updated in single assignments, so it stays
/// consistent.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Emulator {
    /// An empty emulator holding at most `capacity` bytes of object bodies,
    /// with faults placed by `seed`.
    pub fn new(capacity: u64, seed: u64) -> Self {
        Self {
            capacity,
            objects: Mutex::new(Objects::default()),
            control: Mutex::new(S3Op::ALL.map(|op| OpControl {
                // Each kind's generator is seeded apart, from the seed and the kind.
                rng: SplitMix64::new(
                    seed ^ (op.index() as u64).wrapping_mul(0xA24B_AED4_963E_E407),
                ),
                ..OpControl::default()
            })),
            meter: Meter::new(),
        }
    }

    /// Sets how long each request of kind `op` takes.
    pub fn set_latency(&self, op: S3Op, latency: Latency) {
        if let Some(c) = lock(&self.control).get_mut(op.index()) {
            c.latency = latency;
        }
    }

    /// Sets the random failure and throttle rates of kind `op`.
    pub fn set_faults(&self, op: S3Op, faults: Faults) {
        if let Some(c) = lock(&self.control).get_mut(op.index()) {
            c.faults = faults;
        }
    }

    /// Fails the next `n` requests of kind `op` with
    /// [`BlobError::Unavailable`].
    pub fn fail_next(&self, op: S3Op, n: u32) {
        if let Some(c) = lock(&self.control).get_mut(op.index()) {
            c.fail_next = n;
        }
    }

    /// Throttles the next `n` requests of kind `op` with
    /// [`BlobError::Throttled`].
    pub fn throttle_next(&self, op: S3Op, n: u32) {
        if let Some(c) = lock(&self.control).get_mut(op.index()) {
            c.throttle_next = n;
        }
    }

    /// Lets the next `n` requests of kind `op` take effect and then answers
    /// them [`BlobError::Unavailable`], as when S3 commits a write but the
    /// response never reaches the client. A retried conditional create then
    /// finds its own object and gets [`CreateOutcome::AlreadyExists`].
    pub fn lose_response_next(&self, op: S3Op, n: u32) {
        if let Some(c) = lock(&self.control).get_mut(op.index()) {
            c.lose_next = n;
        }
    }

    /// Bytes of object bodies stored now.
    pub fn stored_bytes(&self) -> u64 {
        lock(&self.objects).stored
    }

    /// Objects stored now.
    pub fn object_count(&self) -> usize {
        lock(&self.objects).map.len()
    }

    /// Serves one attempt of `op`: records it, waits its latency, injects
    /// its fault or runs `work`, and records an error if it failed.
    fn attempt<T>(&self, op: S3Op, work: impl FnOnce() -> BlobResult<T>) -> BlobResult<T> {
        self.meter.record(op);
        let decision = lock(&self.control)
            .get_mut(op.index())
            .map(OpControl::decide);
        let Some(Decision {
            delay,
            fault,
            lose_response,
        }) = decision
        else {
            return Err(BlobError::Unavailable);
        };
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        let result = match fault {
            Some(err) => Err(err),
            None if lose_response => work().and(Err(BlobError::Unavailable)),
            None => work(),
        };
        if result.is_err() {
            self.meter.record_error();
        }
        result
    }

    /// Reserves the body's length against the capacity, streams it into a
    /// new allocation, then commits it under the lock. With `if_absent`, a
    /// key taken before the reservation or by the commit refuses the write.
    /// The reservation is given back in one place, under the commit's lock,
    /// whether the body streamed or not.
    fn write(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
        if_absent: bool,
    ) -> BlobResult<CreateOutcome> {
        let len = body.len();
        if !self.reserve(key, len, if_absent)? {
            return Ok(CreateOutcome::AlreadyExists);
        }
        let streamed = stream(body, buf, len);
        let mut objects = lock(&self.objects);
        objects.reserved = objects.reserved.saturating_sub(len);
        let data = streamed?;
        if if_absent && objects.map.contains_key(key) {
            return Ok(CreateOutcome::AlreadyExists);
        }
        // ALLOC: one key, at most MAX_KEY_BYTES, checked by the caller.
        let old = objects.map.insert(key.to_owned(), Arc::new(data));
        let old_len = old.map_or(0, |o| u64::try_from(o.len()).unwrap_or(u64::MAX));
        objects.stored = objects.stored.saturating_sub(old_len).saturating_add(len);
        drop(objects);
        self.meter.add_bytes_written(len);
        Ok(CreateOutcome::Created)
    }

    /// Reserves `len` bytes, counting what is stored and what other PUTs
    /// hold; the object being replaced still counts until the commit. With
    /// `if_absent`, a key already taken answers `false` before capacity is
    /// considered, as S3 answers 412 whatever the body; the commit checks
    /// again for a create that raced this one.
    fn reserve(&self, key: &str, len: u64, if_absent: bool) -> BlobResult<bool> {
        let mut objects = lock(&self.objects);
        if if_absent && objects.map.contains_key(key) {
            return Ok(false);
        }
        let used = objects.stored.saturating_add(objects.reserved);
        match used.checked_add(len) {
            Some(total) if total <= self.capacity => {
                objects.reserved = objects.reserved.saturating_add(len);
                Ok(true)
            }
            _ => Err(BlobError::CapacityExceeded),
        }
    }
}

/// Reads exactly `len` bytes of `body` through `buf` into a new allocation;
/// a body that ends early or runs past `len` is refused.
fn stream(body: &mut dyn ChunkSource, buf: &mut [u8], len: u64) -> BlobResult<Vec<u8>> {
    let cap = usize::try_from(len).map_err(|_| BlobError::CapacityExceeded)?;
    // ALLOC: one object body, at most the emulator's capacity, reserved by the caller.
    let mut data = Vec::with_capacity(cap);
    while data.len() < cap {
        let n = body.fill(buf)?;
        if n == 0 {
            return Err(BlobError::InvalidArgument(
                "body shorter than its declared length",
            ));
        }
        if data.len().saturating_add(n) > cap {
            return Err(BlobError::InvalidArgument(
                "body longer than its declared length",
            ));
        }
        let chunk = buf
            .get(..n)
            .ok_or(BlobError::Stream("source filled past the buffer"))?;
        data.extend_from_slice(chunk);
    }
    if body.fill(buf)? != 0 {
        return Err(BlobError::InvalidArgument(
            "body longer than its declared length",
        ));
    }
    Ok(data)
}

/// The half-open byte span `range` selects in an object of `len` bytes.
fn span(range: ByteRange, len: usize) -> BlobResult<(usize, usize)> {
    match range {
        ByteRange::Full => Ok((0, len)),
        ByteRange::From { start, len: want } => {
            let start = usize::try_from(start).map_err(|_| BlobError::InvalidRange)?;
            if start >= len {
                return Err(BlobError::InvalidRange);
            }
            let want = usize::try_from(want).unwrap_or(usize::MAX);
            Ok((start, start.saturating_add(want).min(len)))
        }
    }
}

impl BlobStore for Emulator {
    fn meter(&self) -> &Meter {
        &self.meter
    }

    fn get(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<GetOutcome> {
        check_key(key)?;
        check_buf(buf)?;
        if let ByteRange::From { len: 0, .. } = range {
            return Err(BlobError::InvalidArgument("range of 0 bytes"));
        }
        self.attempt(range.op(), || {
            let data = lock(&self.objects)
                .map
                .get(key)
                .cloned()
                .ok_or(BlobError::NotFound)?;
            let (start, end) = span(range, data.len())?;
            let body = data.get(start..end).ok_or(BlobError::InvalidRange)?;
            let mut sent = 0u64;
            for piece in body.chunks(buf.len()) {
                let out = buf
                    .get_mut(..piece.len())
                    .ok_or(BlobError::Stream("chunk past the buffer"))?;
                out.copy_from_slice(piece);
                sink.accept(out)?;
                let n = u64::try_from(piece.len()).unwrap_or(u64::MAX);
                self.meter.add_bytes_read(n);
                sent = sent.saturating_add(n);
            }
            Ok(GetOutcome {
                object_len: u64::try_from(data.len()).unwrap_or(u64::MAX),
                bytes: sent,
            })
        })
    }

    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
        check_key(key)?;
        check_buf(buf)?;
        self.attempt(S3Op::Put, || self.write(key, body, buf, false).map(|_| ()))
    }

    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        check_key(key)?;
        check_buf(buf)?;
        let outcome = self.attempt(S3Op::Put, || self.write(key, body, buf, true))?;
        if outcome == CreateOutcome::AlreadyExists {
            // S3 answers 412: a failed attempt, though not an error to the caller.
            self.meter.record_error();
        }
        Ok(outcome)
    }

    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        check_key(key)?;
        self.attempt(S3Op::Head, || {
            Ok(lock(&self.objects).map.get(key).map(|o| ObjectMeta {
                len: u64::try_from(o.len()).unwrap_or(u64::MAX),
            }))
        })
    }

    fn delete(&self, key: &str) -> BlobResult<()> {
        check_key(key)?;
        self.attempt(S3Op::Delete, || {
            let mut objects = lock(&self.objects);
            if let Some(old) = objects.map.remove(key) {
                let n = u64::try_from(old.len()).unwrap_or(u64::MAX);
                objects.stored = objects.stored.saturating_sub(n);
            }
            Ok(())
        })
    }

    // LIST: the emulator's implementation of the trait's one bounded page.
    fn list_prefix(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        max_keys: usize,
        visit: &mut dyn FnMut(&str, u64),
    ) -> BlobResult<ListPage> {
        let max = check_list(prefix, start_after, max_keys)?;
        self.attempt(S3Op::List, || {
            let objects = lock(&self.objects);
            let lower = match start_after {
                Some(after) if after >= prefix => Bound::Excluded(after),
                _ => Bound::Included(prefix),
            };
            let mut matching = objects
                .map
                .range::<str, _>((lower, Bound::Unbounded))
                .take_while(|(k, _)| k.starts_with(prefix));
            let mut keys = 0usize;
            for (k, v) in matching.by_ref().take(max) {
                visit(k, u64::try_from(v.len()).unwrap_or(u64::MAX));
                keys = keys.saturating_add(1);
            }
            Ok(ListPage {
                keys,
                truncated: matching.next().is_some(),
            })
        })
    }
}
