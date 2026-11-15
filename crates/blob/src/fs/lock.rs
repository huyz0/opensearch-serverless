//! The register lock the Java's `FsBlobRegisters.underLock` takes, so Rust
//! and Java processes writing registers in one directory exclude each other
//! (`server/src/main/java/org/opensearch/common/blobstore/fs/FsBlobRegisters.java@3920dc36`).
//!
//! A register maps to one of 64 stripes by the Java `String.hashCode` of
//! its canonical path. A writer holds the stripe's in-process mutex, then
//! an exclusive lock on the stripe's file in the reserved directory beside
//! the register. The file lock is the one Java's `FileChannel.lock` takes:
//! a POSIX record lock over the whole file on Unix (`fcntl(F_SETLKW)`;
//! std's `File::lock` is `flock`, which does not exclude it), `LockFileEx`
//! on Windows. A POSIX record lock belongs to the process and is released
//! when any of its descriptors on the file closes, so only this module
//! opens stripe files, one at a time per stripe, under the stripe's mutex.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::create_dirs;
use super::path::{RESERVED_DIR, io_error};
use crate::error::{BlobError, BlobResult};

/// Stripes per process, and stripe files per directory: the Java's
/// `STRIPE_COUNT`.
pub(crate) const STRIPES: usize = 64;

/// The in-process half of each stripe's lock, shared by every store in the
/// process as the Java's static `STRIPES`.
static MUTEXES: [Mutex<()>; STRIPES] = [const { Mutex::new(()) }; STRIPES];

/// A held register lock: released, file lock then mutex, when dropped.
/// The order is load-bearing: released the other way round, another
/// thread could take the mutex and lock the file again (a POSIX lock this
/// process already holds is granted at once), and this close would then
/// release that thread's lock too, letting another process in.
pub(crate) struct Held {
    file: Option<File>,
    _mutex: MutexGuard<'static, ()>,
}

impl Drop for Held {
    fn drop(&mut self) {
        // Closes the file, releasing its lock, before the guard is dropped
        // with the struct's fields.
        drop(self.file.take());
    }
}

/// The stripe of a register whose canonical path is `path`: the Java's
/// `Math.floorMod(path.toString().hashCode(), 64)`.
pub(crate) fn stripe_of(path: &str) -> usize {
    let hash = path.encode_utf16().fold(0i32, |h, unit| {
        h.wrapping_mul(31).wrapping_add(i32::from(unit))
    });
    usize::try_from(hash.rem_euclid(64)).unwrap_or(0)
}

/// The path Java's `toRealPath` gives for `dir`: canonical, and on Windows
/// without the verbatim prefix Rust's `canonicalize` adds.
fn real_path(dir: &Path) -> BlobResult<String> {
    let real = dir.canonicalize().map_err(|e| io_error(&e))?;
    let text = real.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            // ALLOC: one path per register write.
            return Ok(format!(r"\\{unc}"));
        }
        if let Some(local) = text.strip_prefix(r"\\?\") {
            // ALLOC: one path per register write.
            return Ok(local.to_owned());
        }
    }
    // ALLOC: one path per register write.
    Ok(text.into_owned())
}

/// Takes the lock of the register `name` in `dir`, waiting for any other
/// thread or process holding it. `dir` must exist.
pub(crate) fn lock(dir: &Path, name: &str) -> BlobResult<Held> {
    let real = PathBuf::from(real_path(dir)?);
    let canonical = real.join(name);
    let stripe = stripe_of(&canonical.to_string_lossy());
    let reserved = real.join(RESERVED_DIR);
    create_dirs(&reserved)?;
    let mutex = MUTEXES
        .get(stripe)
        .ok_or(BlobError::Protocol("stripe out of range"))?
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        // ALLOC: one number.
        .open(reserved.join(stripe.to_string()))
        .map_err(|e| io_error(&e))?;
    exclusive(&file).map_err(|e| io_error(&e))?;
    Ok(Held {
        file: Some(file),
        _mutex: mutex,
    })
}

/// Waits for an exclusive lock on the whole file: a POSIX record lock on
/// Unix, `LockFileEx` elsewhere.
pub(crate) fn exclusive(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use rustix::fs::{FlockOperation, fcntl_lock};
        loop {
            match fcntl_lock(file, FlockOperation::LockExclusive) {
                Err(rustix::io::Errno::INTR) => continue,
                other => return other.map_err(io::Error::from),
            }
        }
    }
    #[cfg(not(unix))]
    file.lock()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stripes_match_javas_string_hash_code() {
        // Each stripe as a JVM printed `Math.floorMod(s.hashCode(), 64)`,
        // with the hash: a negative one, and characters outside the BMP,
        // which Java hashes as two UTF-16 units.
        for (path, stripe) in [
            ("", 0),                                                       // 0
            ("a", 33),                                                     // 97
            ("ab", 33),                                                    // 3105
            ("/tmp/x/fixture", 43),                                        // 795449515
            ("/srv/data/indices/\u{fc}n\u{ef}c\u{f6}d\u{e9}/register", 3), // -51255229
            ("/data/\u{1F600}/r", 60),                                     // 646293180
            ("zzzzzzzzzzzz", 0),                                           // 1097476992
        ] {
            assert_eq!(stripe_of(path), stripe, "{path}");
        }
    }
}
