//! Registers on the filesystem store, by the Java's `FsBlobRegisters`
//! protocol (`server/src/main/java/org/opensearch/common/blobstore/fs/FsBlobRegisters.java@3920dc36`),
//! so a Rust and a Java process on one host share registers safely.
//!
//! A register file is the 8-byte big-endian generation, then the value.
//! Every published file is immutable: a write stages a new file in the
//! reserved directory, syncs it, and renames it over the old one, so a
//! reader takes no lock and an open file stays one version. Writers hold
//! the register's lock ([`super::lock`]) while they read the generation and
//! write the next one.
//!
//! Requests are counted as S3 would serve the same operation without
//! remembering versions: a read is one `Get`, a create one `Put`, a swap a
//! `GetRange` for the generation and a `Put` when it applies, a delete a
//! `GetRange` and a `Delete` when it applies.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use super::path::{RESERVED_DIR, check_fs_key, io_error, resolve, staged_name};
use super::{FsStore, create_dirs, discard, is_missing, lock, sync_dir};
use crate::error::{BlobError, BlobResult};
use crate::op::S3Op;
use crate::register::{
    ABSENT_GENERATION, GENERATION_BYTES, RegisterRead, SwapOutcome, check_value, next_generation,
};

impl FsStore {
    /// [`crate::BlobStore::read_register`]: no lock, as the Java's `read`.
    pub(super) fn register_read(
        &self,
        key: &str,
        buf: &mut [u8],
    ) -> BlobResult<Option<RegisterRead>> {
        check_fs_key(key)?;
        let path = resolve(&self.root, key);
        self.attempt(S3Op::Get, || {
            let Some((mut file, len)) = open(&path)? else {
                return Ok(None);
            };
            let Some(value_len) = value_len(len)? else {
                return Ok(None);
            };
            let out = buf.get_mut(..value_len).ok_or(BlobError::InvalidArgument(
                "register larger than the buffer",
            ))?;
            let generation = read_generation(&mut file)?;
            file.read_exact(out).map_err(|e| io_error(&e))?;
            self.meter.add_bytes_read(len);
            Ok(Some(RegisterRead {
                generation,
                len: value_len,
            }))
        })
    }

    /// [`crate::BlobStore::swap_register`] and, with `create`,
    /// [`crate::BlobStore::create_register`], whose generation read is not
    /// counted because S3 sends none.
    pub(super) fn register_swap(
        &self,
        key: &str,
        expected: u64,
        value: &[u8],
        create: bool,
    ) -> BlobResult<SwapOutcome> {
        check_fs_key(key)?;
        check_value(value)?;
        let target = resolve(&self.root, key);
        let (dir, name) = split(&target)?;
        create_dirs(dir)?;
        let _held = lock::lock(dir, name)?;
        let current = if create {
            generation_at(&target)?
        } else {
            self.attempt(S3Op::GetRange, || generation_at(&target))?
        };
        if current != expected {
            if create {
                // As S3's 412 to a create: one refused PUT.
                self.meter.record(S3Op::Put);
                self.meter.record_error();
            }
            return Ok(SwapOutcome::Conflict(Some(current)));
        }
        let next = next_generation(expected)?;
        self.attempt(S3Op::Put, || self.publish(&target, dir, next, value))?;
        Ok(SwapOutcome::Applied(next))
    }

    /// [`crate::BlobStore::delete_register`].
    pub(super) fn register_delete(&self, key: &str, expected: u64) -> BlobResult<bool> {
        check_fs_key(key)?;
        let target = resolve(&self.root, key);
        let (dir, name) = split(&target)?;
        // As the Java: no register file, no lock.
        if fs::symlink_metadata(&target).is_err() {
            self.meter.record(S3Op::GetRange);
            return Ok(false);
        }
        let _held = lock::lock(dir, name)?;
        let current = self.attempt(S3Op::GetRange, || generation_at(&target))?;
        if current == ABSENT_GENERATION || current != expected {
            return Ok(false);
        }
        self.attempt(S3Op::Delete, || {
            fs::remove_file(&target).map_err(|e| io_error(&e))?;
            sync_dir(dir)
        })?;
        Ok(true)
    }

    /// Stages the register file in the reserved directory, syncs it, and
    /// renames it over `target`.
    fn publish(&self, target: &Path, dir: &Path, generation: u64, value: &[u8]) -> BlobResult<()> {
        let staged = dir.join(RESERVED_DIR).join(staged_name());
        let written = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .and_then(|mut file| {
                file.write_all(&generation.to_be_bytes())?;
                file.write_all(value)?;
                file.sync_all()
            })
            .map_err(|e| io_error(&e))
            .and_then(|()| fs::rename(&staged, target).map_err(|e| io_error(&e)));
        if let Err(e) = written {
            discard(&staged);
            return Err(e);
        }
        sync_dir(dir)?;
        let len = u64::try_from(value.len().saturating_add(GENERATION_BYTES)).unwrap_or(u64::MAX);
        self.meter.add_bytes_written(len);
        Ok(())
    }
}

/// The directory a register file is in, and its name.
fn split(target: &Path) -> BlobResult<(&Path, &str)> {
    let dir = target
        .parent()
        .ok_or(BlobError::InvalidArgument("key with no parent"))?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(BlobError::InvalidArgument("key with no name"))?;
    Ok((dir, name))
}

/// The register file at `path` and its length, or `None` when there is
/// none: nothing there, or a directory.
fn open(path: &Path) -> BlobResult<Option<(File, u64)>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if is_missing(&e) => return Ok(None),
        Err(e) => return Err(io_error(&e)),
    };
    let meta = file.metadata().map_err(|e| io_error(&e))?;
    if !meta.is_file() {
        return Ok(None);
    }
    Ok(Some((file, meta.len())))
}

/// The value's length in a register file of `len` bytes, or `None` for an
/// empty file, which the Java reads as no register.
fn value_len(len: u64) -> BlobResult<Option<usize>> {
    if len == 0 {
        return Ok(None);
    }
    let value = len
        .checked_sub(GENERATION_BYTES as u64)
        .ok_or(BlobError::Protocol("register shorter than its generation"))?;
    Ok(Some(usize::try_from(value).unwrap_or(usize::MAX)))
}

/// The generation of the register at `path`, from its first 8 bytes, or
/// [`ABSENT_GENERATION`] when there is none.
fn generation_at(path: &Path) -> BlobResult<u64> {
    let Some((mut file, len)) = open(path)? else {
        return Ok(ABSENT_GENERATION);
    };
    match value_len(len)? {
        None => Ok(ABSENT_GENERATION),
        Some(_) => read_generation(&mut file),
    }
}

fn read_generation(file: &mut File) -> BlobResult<u64> {
    let mut header = [0u8; GENERATION_BYTES];
    file.read_exact(&mut header).map_err(|e| io_error(&e))?;
    Ok(u64::from_be_bytes(header))
}
