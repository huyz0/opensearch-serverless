//! Reading fixture files from disk with a bounded size.
//!
//! Files are read in fixed chunks and refused once they pass
//! [`MAX_FIXTURE_BYTES`]; nothing reads a file whole without a bound
//! (docs/adr/0004-bounded-memory-and-streaming.md).

use std::fmt;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use crate::fixture::{Fixture, FixtureError};

/// The largest fixture file accepted. A fixture holds a few requests and
/// their responses; anything larger is a mistake.
pub const MAX_FIXTURE_BYTES: usize = 1 << 20;

/// The most fixture files one directory tree may hold.
pub const MAX_FIXTURES: usize = 10_000;

/// How deep below the root suites may nest.
const MAX_DEPTH: usize = 4;

const CHUNK: usize = 8 * 1024;

/// Why a fixture could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    /// Reading the file or directory failed.
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The error.
        error: std::io::Error,
    },
    /// The file is larger than the limit.
    TooLarge {
        /// The file.
        path: PathBuf,
        /// The limit it passed, in bytes.
        limit: usize,
    },
    /// The file is not a valid fixture.
    Invalid {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        error: FixtureError,
    },
    /// The tree holds more than [`MAX_FIXTURES`] files, or nests deeper than
    /// the loader walks.
    TooMany {
        /// The directory being walked.
        path: PathBuf,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::TooLarge { path, limit } => {
                write!(
                    f,
                    "{}: larger than the {limit}-byte fixture limit",
                    path.display()
                )
            }
            Self::Invalid { path, error } => write!(f, "{}: {error}", path.display()),
            Self::TooMany { path } => write!(
                f,
                "{}: more than {MAX_FIXTURES} fixtures or nested deeper than {MAX_DEPTH}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for LoadError {}

/// Reads `path` in fixed chunks, refusing it once it passes `limit` bytes.
pub fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, LoadError> {
    let io = |error| LoadError::Io {
        path: path.to_owned(),
        error,
    };
    let mut file = File::open(path).map_err(io)?;
    let mut out = Vec::new();
    let mut chunk = [0u8; CHUNK];
    loop {
        let n = match file.read(&mut chunk) {
            Ok(0) => return Ok(out),
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(io(e)),
        };
        if out.len().saturating_add(n) > limit {
            return Err(LoadError::TooLarge {
                path: path.to_owned(),
                limit,
            });
        }
        out.extend_from_slice(chunk.get(..n).unwrap_or_default());
    }
}

/// Loads and parses one fixture file.
pub fn load_file(path: &Path) -> Result<Fixture, LoadError> {
    let bytes = read_bounded(path, MAX_FIXTURE_BYTES)?;
    Fixture::from_slice(&bytes).map_err(|error| LoadError::Invalid {
        path: path.to_owned(),
        error,
    })
}

/// Loads every `*.json` file under `root`, sorted by path. Other files (the
/// recording script, READMEs) are skipped.
pub fn load_dir(root: &Path) -> Result<Vec<(PathBuf, Fixture)>, LoadError> {
    let mut paths = Vec::new();
    collect(root, 0, MAX_FIXTURES, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|p| load_file(&p).map(|f| (p, f)))
        .collect()
}

fn collect(
    dir: &Path,
    depth: usize,
    max_files: usize,
    out: &mut Vec<PathBuf>,
) -> Result<(), LoadError> {
    let io = |error| LoadError::Io {
        path: dir.to_owned(),
        error,
    };
    let too_many = || LoadError::TooMany {
        path: dir.to_owned(),
    };
    if depth > MAX_DEPTH {
        return Err(too_many());
    }
    for entry in std::fs::read_dir(dir).map_err(io)? {
        let path = entry.map_err(io)?.path();
        if path.is_dir() {
            collect(&path, depth.saturating_add(1), max_files, out)?;
        } else if path.extension().is_some_and(|e| e == "json") {
            if out.len() >= max_files {
                return Err(too_many());
            }
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "load_tests.rs"]
mod tests;
