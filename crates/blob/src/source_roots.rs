//! Where a restore may read another system's repository from: the
//! operator's allow-lists of directories and of `bucket/prefix` roots.
//!
//! Both lists are empty by default, so nothing is readable until an operator
//! names a root. A directory is refused when it is relative, is reached
//! through `..` or through a symlink below its root, or lies outside every
//! root. A bucket and prefix are refused outside every root, and whenever
//! they overlap this deployment's own bucket and prefix, in either
//! direction, because that bucket holds other tenants' data.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Why a source is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootError {
    /// The location is not under any listed root.
    Outside,
    /// The location is relative, or names `..`.
    NotAbsolute,
    /// A step of the location is a symlink.
    Symlink,
    /// The location is not a directory that exists.
    NotDirectory,
    /// The bucket and prefix overlap this deployment's own.
    OwnBucket,
}

impl fmt::Display for RootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Outside => "the location is not under a root this node may read",
            Self::NotAbsolute => "the location must be an absolute path with no '..'",
            Self::Symlink => "the location is reached through a symlink",
            Self::NotDirectory => "the location is not a directory",
            Self::OwnBucket => "the location overlaps this deployment's own bucket and prefix",
        })
    }
}

impl std::error::Error for RootError {}

/// The two allow-lists.
#[derive(Debug, Clone, Default)]
pub struct SourceRoots {
    fs: Vec<PathBuf>,
    s3: Vec<(String, Vec<String>)>,
}

/// The steps of `path` with `.` and empty steps dropped; `None` for `..`.
fn steps(path: &str) -> Option<Vec<String>> {
    let mut out = Vec::new(); // ALLOC: one step a path component.
    for step in path.split('/') {
        match step {
            "" | "." => {}
            ".." => return None,
            // ALLOC: one step, bounded by the path.
            other => out.push(other.to_owned()),
        }
    }
    Some(out)
}

impl SourceRoots {
    /// The lists from their comma-separated forms: directories for `fs`,
    /// `bucket` or `bucket/prefix` for `s3`. A directory root is
    /// canonicalised here, once, as the operator wrote it; one that does not
    /// exist is skipped.
    #[must_use]
    pub fn parse(fs_roots: &str, s3_roots: &str) -> Self {
        // ALLOC: one entry a listed root, operator-sized, once a node.
        let fs = fs_roots
            .split(',')
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .filter_map(|r| Path::new(r).canonicalize().ok())
            .collect(); // ALLOC: as above.
        // ALLOC: one entry a listed root, operator-sized, once a node.
        let s3 = s3_roots
            .split(',')
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .filter_map(|r| {
                let mut parts = steps(r)?.into_iter();
                Some((parts.next()?, parts.collect())) // ALLOC: one prefix a root.
            })
            .collect(); // ALLOC: as above.
        Self { fs, s3 }
    }

    /// The directory `location` names, when it is under a root and no step
    /// below the root is a symlink.
    ///
    /// # Errors
    ///
    /// A [`RootError`].
    pub fn fs_location(&self, location: &str) -> Result<PathBuf, RootError> {
        let path = Path::new(location);
        if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
            return Err(RootError::NotAbsolute);
        }
        // Lexical form: `.` and doubled slashes removed.
        // ALLOC: one path a restore.
        let clean: PathBuf = path.components().collect();
        let root = self
            .fs
            .iter()
            .find(|r| clean.starts_with(r))
            .ok_or(RootError::Outside)?;
        let below = clean.strip_prefix(root).map_err(|_| RootError::Outside)?;
        // ALLOC: one path a restore.
        let mut at = root.clone();
        for step in below.components() {
            at.push(step);
            let meta = std::fs::symlink_metadata(&at).map_err(|_| RootError::NotDirectory)?;
            if meta.file_type().is_symlink() {
                return Err(RootError::Symlink);
            }
        }
        if !at.is_dir() {
            return Err(RootError::NotDirectory);
        }
        Ok(at)
    }

    /// The prefix, ending in `/` (empty for a whole bucket), that `bucket`
    /// and `base_path` name, when they are under a root and do not overlap
    /// `own_bucket` and `own_prefix`.
    ///
    /// # Errors
    ///
    /// A [`RootError`].
    pub fn s3_prefix(
        &self,
        bucket: &str,
        base_path: &str,
        own_bucket: &str,
        own_prefix: &str,
    ) -> Result<String, RootError> {
        let source = steps(base_path).ok_or(RootError::NotAbsolute)?;
        let own = steps(own_prefix).ok_or(RootError::NotAbsolute)?;
        if bucket == own_bucket && (source.starts_with(&own) || own.starts_with(&source)) {
            return Err(RootError::OwnBucket);
        }
        let listed = self
            .s3
            .iter()
            .any(|(b, prefix)| b == bucket && source.starts_with(prefix));
        if !listed {
            return Err(RootError::Outside);
        }
        // ALLOC: one prefix a restore.
        Ok(source.iter().map(|s| format!("{s}/")).collect())
    }
}
