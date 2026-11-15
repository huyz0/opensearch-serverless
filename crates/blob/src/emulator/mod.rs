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

mod body;
pub mod faults;
pub mod http;
mod register;

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::meter::Meter;
use crate::op::S3Op;
use crate::register::known::KnownVersions;
use crate::register::tagged::Precondition;
use crate::register::{RegisterRead, SwapOutcome};
use crate::store::{
    BlobStore, ByteRange, CreateOutcome, GetOutcome, ListPage, ObjectMeta, check_buf, check_key,
    check_list,
};
use body::{span, stream};
use faults::{Decision, Faults, Latency, OpControl, SplitMix64};

/// The bytes a GET is about to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    /// The offset of the first byte.
    pub(crate) first: u64,
    /// How many bytes.
    pub(crate) len: u64,
    /// The whole object's length.
    pub(crate) object_len: u64,
    /// The object's version, for its ETag ([`etag`]).
    pub(crate) tag: u64,
}

/// The ETag of an object's version: quoted, as S3 sends it.
pub(crate) fn etag(tag: u64) -> String {
    // ALLOC: one 18-byte tag per response.
    format!("\"{tag:016x}\"")
}

/// The stored objects and the bytes they, and PUTs in flight, hold.
#[derive(Debug, Default)]
struct Objects {
    map: BTreeMap<String, Arc<Vec<u8>>>,
    /// The writer token stored with an object, as S3 keeps the
    /// `x-amz-meta-writer` header a PUT carried; only the HTTP front end
    /// writes one.
    writers: BTreeMap<String, String>,
    /// Each object's version: every write takes the next one, so two
    /// versions of a key never share an ETag.
    tags: BTreeMap<String, u64>,
    next_tag: u64,
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
    known: KnownVersions,
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
            known: KnownVersions::default(),
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

    /// [`BlobStore::get`], telling `start` the span it will send before the
    /// first chunk: the S3 front end writes its status line and headers
    /// there.
    /// [`BlobStore::put`], storing `writer` as the object's token.
    pub(crate) fn put_as(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
        writer: Option<&str>,
    ) -> BlobResult<()> {
        self.put_if(key, body, buf, None, writer).map(|_| ())
    }

    /// [`BlobStore::create_if_absent`], storing `writer` as the object's
    /// token when it creates one.
    pub(crate) fn create_as(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
        writer: Option<&str>,
    ) -> BlobResult<CreateOutcome> {
        check_key(key)?;
        check_buf(buf)?;
        let written = self.put_if(key, body, buf, Some(Precondition::Absent), writer)?;
        Ok(match written {
            Some(_) => CreateOutcome::Created,
            None => CreateOutcome::AlreadyExists,
        })
    }

    /// A PUT that writes only if `pre` holds, if given, storing `writer` as
    /// the object's token: the new version's tag, or `None` when refused
    /// (S3's 412, recorded as a failed attempt though not an error to the
    /// caller).
    pub(crate) fn put_if(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
        pre: Option<Precondition<'_>>,
        writer: Option<&str>,
    ) -> BlobResult<Option<u64>> {
        check_key(key)?;
        check_buf(buf)?;
        let written = self.attempt(S3Op::Put, || self.write(key, body, buf, pre, writer))?;
        if written.is_none() {
            self.meter.record_error();
        }
        Ok(written)
    }

    /// A DELETE that removes `key` only if its ETag is `etag`: `false`, a
    /// failed attempt as S3's 412, when it is not or there is no object.
    pub(crate) fn delete_if(&self, key: &str, etag_wanted: &str) -> BlobResult<bool> {
        check_key(key)?;
        let deleted = self.attempt(S3Op::Delete, || {
            let mut objects = lock(&self.objects);
            if !holds(&objects, key, Precondition::Matches(etag_wanted)) {
                return Ok(false);
            }
            remove(&mut objects, key);
            Ok(true)
        })?;
        if !deleted {
            self.meter.record_error();
        }
        Ok(deleted)
    }

    /// [`BlobStore::head`], with the object's version and its writer token
    /// if it has one.
    pub(crate) fn head_with_writer(
        &self,
        key: &str,
    ) -> BlobResult<Option<(ObjectMeta, u64, Option<String>)>> {
        check_key(key)?;
        self.attempt(S3Op::Head, || {
            let objects = lock(&self.objects);
            Ok(objects.map.get(key).map(|o| {
                let meta = ObjectMeta {
                    len: u64::try_from(o.len()).unwrap_or(u64::MAX),
                };
                let tag = objects.tags.get(key).copied().unwrap_or(0);
                // ALLOC: one token, bounded by the header line it came in.
                (meta, tag, objects.writers.get(key).cloned())
            }))
        })
    }

    pub(crate) fn get_framed(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        start: &mut dyn FnMut(Span) -> BlobResult<()>,
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<GetOutcome> {
        check_key(key)?;
        check_buf(buf)?;
        if let ByteRange::From { len: 0, .. } = range {
            return Err(BlobError::InvalidArgument("range of 0 bytes"));
        }
        self.attempt(range.op(), || {
            let (data, tag) = {
                let objects = lock(&self.objects);
                let data = objects.map.get(key).cloned().ok_or(BlobError::NotFound)?;
                (data, objects.tags.get(key).copied().unwrap_or(0))
            };
            let (first, end) = span(range, data.len())?;
            let body = data.get(first..end).ok_or(BlobError::InvalidRange)?;
            start(Span {
                first: u64::try_from(first).unwrap_or(u64::MAX),
                len: u64::try_from(body.len()).unwrap_or(u64::MAX),
                object_len: u64::try_from(data.len()).unwrap_or(u64::MAX),
                tag,
            })?;
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
    /// new allocation, then commits it under the lock and returns its tag.
    /// With a precondition, one that fails before the reservation or at the
    /// commit refuses the write (`None`). The reservation is given back in
    /// one place, under the commit's lock, whether the body streamed or not.
    fn write(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
        pre: Option<Precondition<'_>>,
        writer: Option<&str>,
    ) -> BlobResult<Option<u64>> {
        let len = body.len();
        if !self.reserve(key, len, pre)? {
            return Ok(None);
        }
        let streamed = stream(body, buf, len);
        let mut objects = lock(&self.objects);
        objects.reserved = objects.reserved.saturating_sub(len);
        let data = streamed?;
        if pre.is_some_and(|p| !holds(&objects, key, p)) {
            return Ok(None);
        }
        // ALLOC: one key, at most MAX_KEY_BYTES, checked by the caller.
        let old = objects.map.insert(key.to_owned(), Arc::new(data));
        let old_len = old.map_or(0, |o| u64::try_from(o.len()).unwrap_or(u64::MAX));
        objects.stored = objects.stored.saturating_sub(old_len).saturating_add(len);
        objects.next_tag = objects.next_tag.saturating_add(1);
        let tag = objects.next_tag;
        // ALLOC: one key per object, at most MAX_KEY_BYTES.
        objects.tags.insert(key.to_owned(), tag);
        match writer {
            // ALLOC: one key and one token per object, the token bounded by
            // the front end's header line.
            Some(w) => objects.writers.insert(key.to_owned(), w.to_owned()),
            None => objects.writers.remove(key),
        };
        drop(objects);
        self.meter.add_bytes_written(len);
        Ok(Some(tag))
    }

    /// Reserves `len` bytes, counting what is stored and what other PUTs
    /// hold; the object being replaced still counts until the commit. With
    /// a precondition, one that fails answers `false` before capacity is
    /// considered, as S3 answers 412 whatever the body; the commit checks
    /// again for a write that raced this one.
    fn reserve(&self, key: &str, len: u64, pre: Option<Precondition<'_>>) -> BlobResult<bool> {
        let mut objects = lock(&self.objects);
        if pre.is_some_and(|p| !holds(&objects, key, p)) {
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

/// Whether the object at `key` satisfies `pre`.
fn holds(objects: &Objects, key: &str, pre: Precondition<'_>) -> bool {
    match pre {
        Precondition::Absent => !objects.map.contains_key(key),
        Precondition::Matches(wanted) => objects
            .tags
            .get(key)
            .is_some_and(|&tag| etag(tag) == wanted),
    }
}

/// Removes the object at `key` and what is stored with it.
fn remove(objects: &mut Objects, key: &str) {
    objects.writers.remove(key);
    objects.tags.remove(key);
    if let Some(old) = objects.map.remove(key) {
        let n = u64::try_from(old.len()).unwrap_or(u64::MAX);
        objects.stored = objects.stored.saturating_sub(n);
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
        self.get_framed(key, range, buf, &mut |_| Ok(()), sink)
    }

    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
        self.put_as(key, body, buf, None)
    }

    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        self.create_as(key, body, buf, None)
    }

    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        Ok(self.head_with_writer(key)?.map(|(meta, _, _)| meta))
    }

    fn delete(&self, key: &str) -> BlobResult<()> {
        check_key(key)?;
        self.attempt(S3Op::Delete, || {
            remove(&mut lock(&self.objects), key);
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

    fn read_register(&self, key: &str, buf: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
        crate::register::tagged::read(self, key, buf)
    }

    fn swap_register(&self, key: &str, expected: u64, value: &[u8]) -> BlobResult<SwapOutcome> {
        crate::register::tagged::swap(self, key, expected, value)
    }

    fn create_register(&self, key: &str, value: &[u8]) -> BlobResult<SwapOutcome> {
        crate::register::tagged::create(self, key, value)
    }

    fn delete_register(&self, key: &str, expected: u64) -> BlobResult<bool> {
        crate::register::tagged::delete(self, key, expected)
    }
}
