//! Registers over an object store's conditional requests: ETags, with
//! `If-Match` and `If-None-Match: *` on PUT and `If-Match` on DELETE.
//!
//! Ported from `S3BlobContainer`'s `readRegister`,
//! `compareAndSwapRegister`, `createRegisterIfAbsent` and
//! `deleteRegisterIfUnchanged`
//! (`plugins/repository-s3/src/main/java/org/opensearch/repositories/s3/S3BlobContainer.java@3920dc36`).
//! The store's conditional write is the guard: the generation read first
//! only saves a write already known to fail. Where the Java reads the whole
//! register to learn its generation, this reads its first 8 bytes.

use super::known::KnownVersions;
use super::{
    ABSENT_GENERATION, Decoder, GENERATION_BYTES, RegisterRead, RegisterSource, SwapOutcome,
    check_value, generation_of, next_generation,
};
use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::store::{ByteRange, check_key};

/// What a conditional PUT requires of the object already at its key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Precondition<'a> {
    /// No object (`If-None-Match: *`).
    Absent,
    /// The object with this ETag (`If-Match`).
    Matches(&'a str),
}

/// The conditional requests registers are built on. Each call records its
/// attempts in the store's meter as the plain requests do.
pub(crate) trait Tagged {
    /// Streams `range` of `key` into `sink` through `buf` and returns the
    /// object's ETag, or `None` when there is no object.
    fn get_tagged(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<Option<String>>;

    /// Writes `body` to `key` if `pre` holds and returns the new object's
    /// ETag, or `None` when the store refused it because `pre` did not.
    fn put_tagged(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        pre: Precondition<'_>,
    ) -> BlobResult<Option<String>>;

    /// Deletes `key` if its ETag is `etag`; `false` when it was not, or
    /// there was no object.
    fn delete_tagged(&self, key: &str, etag: &str) -> BlobResult<bool>;

    /// The register versions this store remembers.
    fn known(&self) -> &KnownVersions;
}

/// Reads a register's value into `buf`.
pub(crate) fn read(
    store: &dyn Tagged,
    key: &str,
    buf: &mut [u8],
) -> BlobResult<Option<RegisterRead>> {
    check_key(key)?;
    let mut decoder = Decoder::new(buf);
    let mut chunk = [0u8; 4096];
    let mut sink = |c: &[u8]| decoder.feed(c);
    match store.get_tagged(key, ByteRange::Full, &mut chunk, &mut sink)? {
        None => {
            store.known().forget(key);
            Ok(None)
        }
        Some(etag) => {
            let read = decoder.finish()?;
            match read {
                Some(r) => store.known().remember(key, r.generation, &etag),
                None => store.known().forget(key),
            }
            Ok(read)
        }
    }
}

/// The generation and ETag of the register at `key`, from its first 8
/// bytes, or `None` when there is none.
fn current(store: &dyn Tagged, key: &str) -> BlobResult<Option<(u64, String)>> {
    let mut prefix = [0u8; GENERATION_BYTES];
    let mut filled = 0usize;
    let mut sink = |c: &[u8]| {
        let end = filled.saturating_add(c.len());
        prefix
            .get_mut(filled..end)
            .ok_or(BlobError::Protocol("ranged read past its range"))?
            .copy_from_slice(c);
        filled = end;
        Ok(())
    };
    let range = ByteRange::From {
        start: 0,
        len: GENERATION_BYTES as u64,
    };
    let mut buf = [0u8; GENERATION_BYTES];
    match store.get_tagged(key, range, &mut buf, &mut sink) {
        Ok(None) => Ok(None),
        Ok(Some(etag)) => Ok(Some((
            generation_of(prefix.get(..filled).unwrap_or_default())?,
            etag,
        ))),
        // An empty object: the Java's filesystem reads it as absent, but
        // nothing can swap from it here without an ETag to match.
        Err(BlobError::InvalidRange) => {
            Err(BlobError::Protocol("register shorter than its generation"))
        }
        Err(e) => Err(e),
    }
}

/// Writes `value` at `generation` under `pre`, remembering the result.
fn write(
    store: &dyn Tagged,
    key: &str,
    generation: u64,
    value: &[u8],
    pre: Precondition<'_>,
) -> BlobResult<SwapOutcome> {
    check_value(value)?;
    let mut body = RegisterSource::new(generation, value);
    match store.put_tagged(key, &mut body, pre) {
        Ok(Some(etag)) => {
            store.known().remember(key, generation, &etag);
            Ok(SwapOutcome::Applied(generation))
        }
        Ok(None) => {
            store.known().forget(key);
            Ok(SwapOutcome::Conflict(None))
        }
        Err(e) => {
            store.known().forget(key);
            Err(e)
        }
    }
}

/// Replaces the register's value if it holds `expected`.
pub(crate) fn swap(
    store: &dyn Tagged,
    key: &str,
    expected: u64,
    value: &[u8],
) -> BlobResult<SwapOutcome> {
    check_key(key)?;
    check_value(value)?;
    if expected != ABSENT_GENERATION
        && let Some(known) = store.known().get(key).filter(|k| k.generation == expected)
    {
        let next = next_generation(expected)?;
        return write(store, key, next, value, Precondition::Matches(&known.etag));
    }
    let (generation, etag) = match current(store, key)? {
        Some((g, e)) => (g, Some(e)),
        None => (ABSENT_GENERATION, None),
    };
    if generation != expected {
        return Ok(SwapOutcome::Conflict(Some(generation)));
    }
    let next = next_generation(expected)?;
    let pre = etag
        .as_deref()
        .map_or(Precondition::Absent, Precondition::Matches);
    write(store, key, next, value, pre)
}

/// Creates the register with `value` if it does not exist, in one request.
pub(crate) fn create(store: &dyn Tagged, key: &str, value: &[u8]) -> BlobResult<SwapOutcome> {
    check_key(key)?;
    check_value(value)?;
    let generation = next_generation(ABSENT_GENERATION)?;
    write(store, key, generation, value, Precondition::Absent)
}

/// Deletes the register if it still holds `expected`.
pub(crate) fn delete(store: &dyn Tagged, key: &str, expected: u64) -> BlobResult<bool> {
    check_key(key)?;
    let etag = match store.known().get(key).filter(|k| k.generation == expected) {
        Some(known) => known.etag,
        None => match current(store, key)? {
            Some((g, etag)) if g == expected => etag,
            _ => return Ok(false),
        },
    };
    let deleted = store.delete_tagged(key, &etag);
    store.known().forget(key);
    deleted
}
