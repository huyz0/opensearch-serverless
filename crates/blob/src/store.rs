//! The object-store interface every node role uses.
//!
//! Ported from the operations the Java shell's `MeteredBlobStore`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/store/MeteredBlobStore.java@bf347adb`)
//! counts on its `BlobContainer`: `readBlob` whole and ranged become
//! [`BlobStore::get`], `writeBlob` becomes [`BlobStore::put`], `writeBlob`
//! with `failIfAlreadyExists` becomes [`BlobStore::create_if_absent`],
//! `blobExists` becomes [`BlobStore::head`], `deleteBlobsIgnoringIfNotExists`
//! becomes [`BlobStore::delete`], and the bounded
//! `listBlobsByPrefix(prefix, startAfter, limit)` becomes
//! [`BlobStore::list_prefix`]. The unbounded listings (`listBlobs`,
//! `children`) are not ported: every listing here is one page.
//!
//! Differences from the Java, by design (ADR 0004, ADR 0005): calls block;
//! bodies stream through a caller-provided buffer instead of an
//! `InputStream`; each implementation records its own attempts in its
//! [`Meter`] rather than being wrapped by a counting decorator, because only
//! the code that sends a request sees its retries.

use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::meter::Meter;
use crate::op::S3Op;
use crate::register::{RegisterRead, SwapOutcome};

/// The most keys one LIST page returns, as S3's `ListObjectsV2` caps it.
pub const MAX_LIST_KEYS: usize = 1000;

/// The longest key S3 accepts, in bytes.
pub const MAX_KEY_BYTES: usize = 1024;

/// Which bytes of an object a GET asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByteRange {
    /// The whole object: a plain GET.
    Full,
    /// `len` bytes from `start`, fewer where the object ends first: a GET
    /// with `Range: bytes=start-(start+len-1)`.
    From {
        /// The first byte.
        start: u64,
        /// How many bytes, at least 1.
        len: u64,
    },
}

impl ByteRange {
    /// The request kind this GET is billed and counted as.
    pub const fn op(self) -> S3Op {
        match self {
            ByteRange::Full => S3Op::Get,
            ByteRange::From { .. } => S3Op::GetRange,
        }
    }
}

/// What a GET delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetOutcome {
    /// The whole object's length.
    pub object_len: u64,
    /// The bytes handed to the sink.
    pub bytes: u64,
}

/// How a conditional create ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateOutcome {
    /// The object did not exist and now holds the body.
    Created,
    /// The key was taken by another writer (S3 412 `PreconditionFailed`);
    /// this call changed nothing. This is how a fenced-out writer learns it
    /// lost. A store that retries a create makes sure of it: when a retry
    /// is refused after an attempt whose outcome is unknown, it reads which
    /// writer the object came from, and answers [`CreateOutcome::Created`]
    /// if it was this call. A caller that repeats a failed call itself
    /// makes a new call, which cannot see the old one's object as its own.
    AlreadyExists,
}

/// What a HEAD reports about an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectMeta {
    /// The object's length in bytes.
    pub len: u64,
}

/// What one LIST page returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListPage {
    /// How many keys the page visited.
    pub keys: usize,
    /// Whether more keys match after the last one visited; ask again with
    /// that key as `start_after`.
    pub truncated: bool,
}

/// An object store: S3, or the in-process emulator.
///
/// Every method that reaches the store records exactly one attempt of its
/// [`S3Op`] in [`BlobStore::meter`] (more if it retries, one per attempt),
/// and one error for each attempt that fails. Argument checks happen first
/// and record nothing.
///
/// Retries: a PUT may be retried after [`ChunkSource::rewind`]; a GET is
/// retried only before its first chunk reaches the sink, so a sink never
/// sees a byte twice. A failure after that is returned to the caller.
pub trait BlobStore: Send + Sync {
    /// The meter this store records its attempts in.
    fn meter(&self) -> &Meter;

    /// Streams `range` of `key` into `sink`, at most `buf.len()` bytes at a
    /// time, through `buf`. A missing key is [`BlobError::NotFound`]; a range
    /// starting at or past the end is [`BlobError::InvalidRange`].
    fn get(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<GetOutcome>;

    /// Writes `body` to `key`, replacing any object there, reading it
    /// through `buf`. The object appears whole or not at all.
    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()>;

    /// Writes `body` to `key` only if no object is there
    /// (`If-None-Match: *`): the fencing primitive. Of racing creates,
    /// exactly one body is stored, and at most one call gets
    /// [`CreateOutcome::Created`], the one whose body it is. A call that
    /// fails (an error, not an outcome) may still have stored its body.
    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome>;

    /// The object's metadata, or `None` if `key` does not exist.
    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>>;

    /// Deletes `key`. Deleting a missing key succeeds, as on S3.
    fn delete(&self, key: &str) -> BlobResult<()>;

    /// Visits up to `min(max_keys, MAX_LIST_KEYS)` keys starting with
    /// `prefix`, in byte order, after `start_after` if given, with each
    /// object's length. `visit` must not call back into the store.
    // LIST: the trait's one listing, a single bounded page; each caller
    // carries its own justification.
    fn list_prefix(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        max_keys: usize,
        visit: &mut dyn FnMut(&str, u64),
    ) -> BlobResult<ListPage>;

    /// Reads the register at `key` into the front of `buf`: its generation
    /// and value length, or `None` when there is no register. A register
    /// that does not fit `buf` is [`BlobError::InvalidArgument`]; an object
    /// shorter than a generation is [`BlobError::Protocol`].
    fn read_register(&self, key: &str, buf: &mut [u8]) -> BlobResult<Option<RegisterRead>>;

    /// Replaces the register's value with `value` if it holds generation
    /// `expected` ([`crate::register::ABSENT_GENERATION`] for none), and
    /// moves it to the next generation. Of racing swaps from one
    /// generation, at most one applies.
    fn swap_register(&self, key: &str, expected: u64, value: &[u8]) -> BlobResult<SwapOutcome>;

    /// Creates the register with `value` at a new random generation if
    /// there is none: one request.
    fn create_register(&self, key: &str, value: &[u8]) -> BlobResult<SwapOutcome>;

    /// Deletes the register if it holds generation `expected`; `false` when
    /// it held another or none.
    fn delete_register(&self, key: &str, expected: u64) -> BlobResult<bool>;
}

/// Checks a key as S3 would: non-empty and at most [`MAX_KEY_BYTES`].
pub fn check_key(key: &str) -> BlobResult<()> {
    if key.is_empty() {
        return Err(BlobError::InvalidArgument("empty key"));
    }
    if key.len() > MAX_KEY_BYTES {
        return Err(BlobError::InvalidArgument("key longer than 1024 bytes"));
    }
    Ok(())
}

/// Checks a body buffer: a store cannot stream through an empty one.
pub fn check_buf(buf: &[u8]) -> BlobResult<()> {
    if buf.is_empty() {
        return Err(BlobError::InvalidArgument("empty buffer"));
    }
    Ok(())
}

/// Checks a LIST request's prefix, cursor and page size, and returns the
/// page size capped at [`MAX_LIST_KEYS`].
pub fn check_list(prefix: &str, start_after: Option<&str>, max_keys: usize) -> BlobResult<usize> {
    if max_keys == 0 {
        return Err(BlobError::InvalidArgument("max_keys of 0"));
    }
    if prefix.len() > MAX_KEY_BYTES || start_after.is_some_and(|s| s.len() > MAX_KEY_BYTES) {
        return Err(BlobError::InvalidArgument(
            "prefix or cursor longer than 1024 bytes",
        ));
    }
    Ok(max_keys.min(MAX_LIST_KEYS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ranged_get_is_counted_apart_from_a_whole_one() {
        assert_eq!(ByteRange::Full.op(), S3Op::Get);
        assert_eq!(ByteRange::From { start: 0, len: 1 }.op(), S3Op::GetRange);
    }

    #[test]
    fn keys_are_non_empty_and_at_most_1024_bytes() {
        assert!(check_key("a").is_ok());
        assert!(check_key(&"k".repeat(MAX_KEY_BYTES)).is_ok());
        assert_eq!(check_key(""), Err(BlobError::InvalidArgument("empty key")));
        assert!(check_key(&"k".repeat(MAX_KEY_BYTES + 1)).is_err());
    }

    #[test]
    fn list_pages_are_capped_at_1000() {
        assert_eq!(check_list("", None, 5), Ok(5));
        assert_eq!(check_list("p/", Some("p/a"), 10_000), Ok(MAX_LIST_KEYS));
        assert!(check_list("", None, 0).is_err());
        assert!(check_list(&"p".repeat(MAX_KEY_BYTES + 1), None, 1).is_err());
        assert!(check_list("", Some(&"c".repeat(MAX_KEY_BYTES + 1)), 1).is_err());
        let longest = "k".repeat(MAX_KEY_BYTES);
        assert_eq!(check_list(&longest, Some(&longest), 1), Ok(1));
    }

    #[test]
    fn an_empty_buffer_is_refused() {
        assert!(check_buf(&[]).is_err());
        assert!(check_buf(&[0]).is_ok());
    }
}
