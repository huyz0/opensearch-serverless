//! Where a key lives in the store's directory, and how filesystem errors
//! read as store errors.
//!
//! Ported from the shell's `FsBlobStore.buildPath` and `validateKey`, and
//! `FsBlobContainer.resolveBlobPath`
//! (`server/src/main/java/org/opensearch/common/blobstore/fs/FsBlobContainer.java@3920dc36`):
//! each `/`-separated part of a key is one path component of at most 255
//! bytes, and nothing may name the reserved register-lock directory. A key
//! here has at most 64 components, which the Java does not limit.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{BlobError, BlobResult};
use crate::store::check_key;

/// The directory beside the blobs that the Java keeps register locks and
/// staged register files in, and that both sides' listings skip. Staged
/// blobs go here too, so no listing ever sees one.
pub(crate) const RESERVED_DIR: &str = ".opensearch-register-locks#";

/// The longest path component the Java's `validateComponent` accepts, in
/// UTF-8 bytes.
pub(crate) const MAX_COMPONENT_BYTES: usize = 255;

/// The most `/`-separated components a key may have here. A LIST walk
/// holds one open directory per level, so depth bounds its descriptors;
/// the shell keys in the fixtures have at most four.
pub(crate) const MAX_COMPONENTS: usize = 64;

/// The prefix of a staged file's name, the Java's `TEMP_FILE_PREFIX`.
const STAGED_PREFIX: &str = "pending-";

/// Checks `key` as S3 would ([`check_key`]) and as a filesystem must: every
/// component non-empty, not `.` or `..`, at most 255 bytes, free of `\` and
/// NUL, and not the reserved directory.
pub(crate) fn check_fs_key(key: &str) -> BlobResult<()> {
    check_key(key)?;
    if key.split('/').count() > MAX_COMPONENTS {
        return Err(BlobError::InvalidArgument(
            "key of more than 64 components on a filesystem store",
        ));
    }
    key.split('/').try_for_each(check_component)
}

fn check_component(part: &str) -> BlobResult<()> {
    if part.is_empty() || part == "." || part == ".." {
        return Err(BlobError::InvalidArgument(
            "empty, '.' or '..' key component on a filesystem store",
        ));
    }
    if part.len() > MAX_COMPONENT_BYTES {
        return Err(BlobError::InvalidArgument(
            "key component longer than 255 bytes",
        ));
    }
    if part.contains(['\\', '\0']) {
        return Err(BlobError::InvalidArgument(
            "key component holding '\\' or NUL",
        ));
    }
    if part == RESERVED_DIR {
        return Err(BlobError::InvalidArgument(
            "key inside the reserved register-lock directory",
        ));
    }
    Ok(())
}

/// The file `key` is stored in under `root`. The key must have passed
/// [`check_fs_key`].
pub(crate) fn resolve(root: &Path, key: &str) -> PathBuf {
    // ALLOC: one path per call, the root plus a key of at most MAX_KEY_BYTES.
    let mut path = root.to_path_buf();
    path.extend(key.split('/'));
    path
}

/// A fresh name for a staged file: unique within this process by a counter,
/// and across processes by the process id and the clock. The caller creates
/// it exclusively, so a collision fails rather than shares a file.
pub(crate) fn staged_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    // ALLOC: one name per write, under 80 bytes.
    format!("{STAGED_PREFIX}{:x}-{nanos:x}-{n:x}", std::process::id())
}

/// What a filesystem error means to a store caller. Not found is the
/// caller's to interpret, so it is mapped where the call knows what is
/// missing; here it means the store's directory went away.
pub(crate) fn io_error(err: &io::Error) -> BlobError {
    match err.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => BlobError::Denied,
        io::ErrorKind::NotADirectory
        | io::ErrorKind::IsADirectory
        | io::ErrorKind::DirectoryNotEmpty
        | io::ErrorKind::AlreadyExists => {
            BlobError::InvalidArgument("key collides with a key that is also a prefix of others")
        }
        io::ErrorKind::InvalidFilename => {
            BlobError::InvalidArgument("key the filesystem cannot name")
        }
        io::ErrorKind::Unsupported => BlobError::InvalidArgument(
            "filesystem cannot hard-link, so it cannot create atomically",
        ),
        _ => BlobError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_one_component_per_part() {
        let p = resolve(Path::new("r"), "a/b#c/t=2");
        let parts: Vec<_> = p.components().map(|c| c.as_os_str().to_owned()).collect();
        assert_eq!(parts, ["r", "a", "b#c", "t=2"]);
    }

    #[test]
    fn components_are_checked() {
        assert!(check_fs_key("indices/fixture").is_ok());
        assert!(check_fs_key(&"c".repeat(MAX_COMPONENT_BYTES)).is_ok());
        for bad in [
            "",
            "a//b",
            "/a",
            "a/",
            "a/./b",
            "../a",
            "a\\b",
            "a\0b",
            ".opensearch-register-locks#/x",
        ] {
            assert!(
                matches!(check_fs_key(bad), Err(BlobError::InvalidArgument(_))),
                "{bad:?}"
            );
        }
        assert!(check_fs_key(&"c".repeat(MAX_COMPONENT_BYTES + 1)).is_err());
        let deepest = vec!["d"; MAX_COMPONENTS].join("/");
        assert!(check_fs_key(&deepest).is_ok());
        assert!(check_fs_key(&format!("{deepest}/d")).is_err());
    }

    #[test]
    fn staged_names_are_distinct_and_marked() {
        let a = staged_name();
        let b = staged_name();
        assert_ne!(a, b);
        assert!(a.starts_with(STAGED_PREFIX));
    }

    #[test]
    fn filesystem_errors_map_to_store_errors() {
        use io::ErrorKind as K;
        let map = |k: K| io_error(&io::Error::from(k));
        assert_eq!(map(K::PermissionDenied), BlobError::Denied);
        assert_eq!(map(K::ReadOnlyFilesystem), BlobError::Denied);
        for k in [
            K::NotADirectory,
            K::IsADirectory,
            K::DirectoryNotEmpty,
            K::AlreadyExists,
        ] {
            assert!(matches!(map(k), BlobError::InvalidArgument(_)), "{k:?}");
        }
        assert!(matches!(
            map(K::InvalidFilename),
            BlobError::InvalidArgument(_)
        ));
        assert!(matches!(map(K::Unsupported), BlobError::InvalidArgument(_)));
        assert_eq!(map(K::StorageFull), BlobError::Unavailable);
        assert_eq!(map(K::Other), BlobError::Unavailable);
    }
}
