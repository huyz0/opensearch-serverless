//! A [`BlobStore`] over a directory: `serverless.store.type = fs`.
//!
//! Ported from the shell's `FsBlobContainer` and `FsBlobStore`
//! (`server/src/main/java/org/opensearch/common/blobstore/fs/FsBlobContainer.java@3920dc36`),
//! with the same key layout, so a directory one side wrote the other reads:
//! the key `a/b/c` is the file `<root>/a/b/c`.
//!
//! A key is never seen part-written, as on S3. A body is staged in a file
//! of its own and synced, then [`BlobStore::put`] renames it over the key
//! and [`BlobStore::create_if_absent`] hard-links it to the key, which
//! fails when the name is taken: the Java's `createWhole`, which it adopted
//! after a reader took a record still being written for a zero-length
//! fence. Staged files sit in the reserved directory beside the key, which
//! both sides' listings skip.
//!
//! Every call records one attempt of its S3 kind, as the Java's
//! `MeteredBlobStore` counts its filesystem store, so a request budget
//! measured here means what it means on S3. Nothing is retried: a local
//! filesystem has no transient failure a retry would cure.
//!
//! Registers follow the Java's `FsBlobRegisters` protocol, file locks
//! included (`register.rs`), so they are shared safely with a Java process.
//!
//! Single host only. A shared filesystem gives no atomic link or rename
//! across hosts, and the Java documents the same limit.

mod list;
mod lock;
mod path;
mod register;

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::meter::Meter;
use crate::op::S3Op;
use crate::register::{RegisterRead, SwapOutcome};
use crate::store::{
    BlobStore, ByteRange, CreateOutcome, GetOutcome, ListPage, ObjectMeta, check_buf, check_list,
};
use path::{RESERVED_DIR, check_fs_key, io_error, resolve, staged_name};

/// An object store kept in a directory, with S3's semantics where a
/// filesystem can give them.
#[derive(Debug)]
pub struct FsStore {
    root: PathBuf,
    meter: Meter,
}

impl FsStore {
    /// A store over `root`, created if missing, as the Java's writable
    /// `FsBlobStore` does.
    pub fn new(root: impl Into<PathBuf>) -> BlobResult<Self> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| io_error(&e))?;
        Ok(Self {
            root,
            meter: Meter::new(),
        })
    }

    /// The directory the store keeps its objects in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Serves one attempt of `op`: records it, runs `work`, and records an
    /// error if it failed.
    fn attempt<T>(&self, op: S3Op, work: impl FnOnce() -> BlobResult<T>) -> BlobResult<T> {
        self.meter.record(op);
        let result = work();
        if result.is_err() {
            self.meter.record_error();
        }
        result
    }

    /// Writes `body` to a new staged file beside `target` and syncs it.
    /// The staged file is removed again if anything fails.
    fn stage(
        &self,
        target: &Path,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<PathBuf> {
        let parent = target
            .parent()
            .ok_or(BlobError::InvalidArgument("key with no parent"))?;
        let reserved = parent.join(RESERVED_DIR);
        create_dirs(&reserved)?;
        let staged = reserved.join(staged_name());
        let written = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .map_err(|e| io_error(&e))
            .and_then(|mut file| {
                copy_exact(body, buf, &mut file)?;
                file.sync_all().map_err(|e| io_error(&e))
            });
        match written {
            Ok(()) => Ok(staged),
            Err(e) => {
                discard(&staged);
                Err(e)
            }
        }
    }
}

/// Copies exactly `body.len()` bytes of `body` through `buf` into `out`; a
/// body that ends early or runs past its length is refused.
fn copy_exact(body: &mut dyn ChunkSource, buf: &mut [u8], out: &mut File) -> BlobResult<()> {
    let len = body.len();
    let mut left = len;
    loop {
        let n = body.fill(buf)?;
        if n == 0 {
            break;
        }
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        left = left.checked_sub(n64).ok_or(BlobError::InvalidArgument(
            "body longer than its declared length",
        ))?;
        let chunk = buf
            .get(..n)
            .ok_or(BlobError::Stream("source filled past the buffer"))?;
        out.write_all(chunk).map_err(|e| io_error(&e))?;
    }
    if left > 0 {
        return Err(BlobError::InvalidArgument(
            "body shorter than its declared length",
        ));
    }
    Ok(())
}

/// Creates `dir` and any missing directory above it, then syncs the
/// parent of each one it created, so a new key's directories survive a
/// power failure with the key, on a filesystem that does not journal them.
fn create_dirs(dir: &Path) -> BlobResult<()> {
    let mut first_new = None;
    let mut at = Some(dir);
    while let Some(d) = at {
        if d.is_dir() {
            break;
        }
        first_new = Some(d);
        at = d.parent();
    }
    let Some(first_new) = first_new else {
        return Ok(());
    };
    fs::create_dir_all(dir).map_err(|e| io_error(&e))?;
    // The parents of `dir` up to and including the one that held `first_new`.
    let mut created = Some(dir);
    while let Some(d) = created {
        if let Some(parent) = d.parent() {
            sync_dir(parent)?;
        }
        if d == first_new {
            break;
        }
        created = d.parent();
    }
    Ok(())
}

/// Removes a staged file, ignoring failure: one left behind is invisible
/// to every listing.
fn discard(staged: &Path) {
    let _ = fs::remove_file(staged);
}

/// Makes a directory's new entries durable. Windows has no directory
/// handle to sync, and the Java's `IOUtils.fsync` skips it there too.
fn sync_dir(dir: &Path) -> BlobResult<()> {
    #[cfg(unix)]
    {
        File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(|e| io_error(&e))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Whether `err` means there is no object at the key: nothing there, or a
/// file where a directory on the way would be.
fn is_missing(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// The half-open byte span `range` selects in an object of `len` bytes.
fn span(range: ByteRange, len: u64) -> BlobResult<(u64, u64)> {
    match range {
        ByteRange::Full => Ok((0, len)),
        ByteRange::From { start, len: want } => {
            if start >= len {
                return Err(BlobError::InvalidRange);
            }
            Ok((start, start.saturating_add(want).min(len)))
        }
    }
}

impl BlobStore for FsStore {
    fn meter(&self) -> &Meter {
        &self.meter
    }

    fn validate_key(&self, key: &str) -> BlobResult<()> {
        check_fs_key(key)
    }

    fn get(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<GetOutcome> {
        check_fs_key(key)?;
        check_buf(buf)?;
        if let ByteRange::From { len: 0, .. } = range {
            return Err(BlobError::InvalidArgument("range of 0 bytes"));
        }
        let path = resolve(&self.root, key);
        self.attempt(range.op(), || {
            let mut file = match File::open(&path) {
                Ok(file) => file,
                Err(e) if is_missing(&e) || path.is_dir() => return Err(BlobError::NotFound),
                Err(e) => return Err(io_error(&e)),
            };
            let meta = file.metadata().map_err(|e| io_error(&e))?;
            if !meta.is_file() {
                return Err(BlobError::NotFound);
            }
            let object_len = meta.len();
            let (first, end) = span(range, object_len)?;
            file.seek(SeekFrom::Start(first))
                .map_err(|e| io_error(&e))?;
            let mut left = end.saturating_sub(first);
            let mut sent = 0u64;
            while left > 0 {
                let want = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
                let out = buf
                    .get_mut(..want)
                    .ok_or(BlobError::Stream("chunk past the buffer"))?;
                let n = match file.read(out) {
                    Ok(0) => return Err(BlobError::Unavailable),
                    Ok(n) => n,
                    Err(e) => return Err(io_error(&e)),
                };
                let chunk = out
                    .get(..n)
                    .ok_or(BlobError::Stream("read past the buffer"))?;
                sink.accept(chunk)?;
                let n64 = u64::try_from(n).unwrap_or(u64::MAX);
                self.meter.add_bytes_read(n64);
                sent = sent.saturating_add(n64);
                left = left.saturating_sub(n64);
            }
            Ok(GetOutcome {
                object_len,
                bytes: sent,
            })
        })
    }

    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
        check_fs_key(key)?;
        check_buf(buf)?;
        let target = resolve(&self.root, key);
        self.attempt(S3Op::Put, || {
            let staged = self.stage(&target, body, buf)?;
            if let Err(e) = fs::rename(&staged, &target) {
                discard(&staged);
                return Err(io_error(&e));
            }
            sync_dir(target.parent().unwrap_or(&self.root))?;
            self.meter.add_bytes_written(body.len());
            Ok(())
        })
    }

    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        check_fs_key(key)?;
        check_buf(buf)?;
        let target = resolve(&self.root, key);
        let outcome = self.attempt(S3Op::Put, || {
            // A taken key is refused before the body is written, as S3
            // answers 412 whatever the body; the link below decides a race.
            if fs::symlink_metadata(&target).is_ok() {
                return Ok(CreateOutcome::AlreadyExists);
            }
            let staged = self.stage(&target, body, buf)?;
            let linked = fs::hard_link(&staged, &target);
            discard(&staged);
            if let Err(e) = linked {
                // The name was taken since the check above: another create won.
                if fs::symlink_metadata(&target).is_ok() {
                    return Ok(CreateOutcome::AlreadyExists);
                }
                return Err(io_error(&e));
            }
            sync_dir(target.parent().unwrap_or(&self.root))?;
            self.meter.add_bytes_written(body.len());
            Ok(CreateOutcome::Created)
        })?;
        if outcome == CreateOutcome::AlreadyExists {
            // As S3's 412: a failed attempt, though not an error to the caller.
            self.meter.record_error();
        }
        Ok(outcome)
    }

    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        check_fs_key(key)?;
        let path = resolve(&self.root, key);
        self.attempt(S3Op::Head, || match fs::metadata(&path) {
            Ok(meta) if meta.is_file() => Ok(Some(ObjectMeta { len: meta.len() })),
            Ok(_) => Ok(None),
            Err(e) if is_missing(&e) => Ok(None),
            Err(e) => Err(io_error(&e)),
        })
    }

    fn delete(&self, key: &str) -> BlobResult<()> {
        check_fs_key(key)?;
        let path = resolve(&self.root, key);
        self.attempt(S3Op::Delete, || match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if is_missing(&e) => Ok(()),
            // A directory is no object: deleting it changes nothing, as a
            // DELETE of a key that is only a prefix does on S3.
            Err(_) if path.is_dir() => Ok(()),
            Err(e) => Err(io_error(&e)),
        })
    }

    // LIST: the filesystem store's implementation of the trait's one bounded page.
    fn list_prefix(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        max_keys: usize,
        visit: &mut dyn FnMut(&str, u64),
    ) -> BlobResult<ListPage> {
        let max = check_list(prefix, start_after, max_keys)?;
        self.attempt(S3Op::List, || {
            let page = list::page(&self.root, prefix, start_after, max)?;
            for (key, len) in &page.keys {
                visit(key, *len);
            }
            Ok(ListPage {
                keys: page.keys.len(),
                truncated: page.truncated,
            })
        })
    }

    fn read_register(&self, key: &str, buf: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
        self.register_read(key, buf)
    }

    fn swap_register(&self, key: &str, expected: u64, value: &[u8]) -> BlobResult<SwapOutcome> {
        self.register_swap(key, expected, value, false)
    }

    fn create_register(&self, key: &str, value: &[u8]) -> BlobResult<SwapOutcome> {
        self.register_swap(key, crate::register::ABSENT_GENERATION, value, true)
    }

    fn delete_register(&self, key: &str, expected: u64) -> BlobResult<bool> {
        self.register_delete(key, expected)
    }
}
