//! One LIST page over a directory tree, in S3's order.
//!
//! S3 lists a flat key space in byte order; a filesystem holds the same
//! keys as nested directories, whose entries come back in no order. The
//! Java's `FsBlobContainer.listBlobsByPrefix(prefix, startAfter, limit)`
//! (`server/src/main/java/org/opensearch/common/blobstore/fs/FsBlobContainer.java@3920dc36`)
//! solves this for one directory with a bounded heap of the smallest names
//! after the cursor; this walk does the same across the directories under
//! the prefix, comparing whole keys, so `a/b-c` sorts before `a/b/c` as it
//! does on S3. It holds at most `max + 1` keys whatever the tree's size.

use std::collections::BinaryHeap;
use std::fs;
use std::io;
use std::path::Path;

use super::path::{MAX_COMPONENTS, RESERVED_DIR, io_error};
use crate::error::BlobResult;
use crate::store::MAX_KEY_BYTES;

/// One page: the keys visited, with sizes, in byte order, and whether more
/// match after them.
pub(crate) struct Page {
    pub(crate) keys: Vec<(String, u64)>,
    pub(crate) truncated: bool,
}

/// The smallest keys seen so far, at most `cap` of them.
struct Smallest<'a> {
    heap: BinaryHeap<(String, u64)>,
    cap: usize,
    start_after: Option<&'a str>,
}

impl Smallest<'_> {
    fn full(&self) -> bool {
        self.heap.len() >= self.cap
    }

    /// Whether a key `key` could still enter the page.
    fn admits(&self, key: &str) -> bool {
        if self.start_after.is_some_and(|after| key <= after) {
            return false;
        }
        !self.full()
            || self
                .heap
                .peek()
                .is_some_and(|(largest, _)| key < largest.as_str())
    }

    /// Whether some key under the directory whose keys all start with
    /// `dir` (ending in `/`, `depth` components) could still enter the page.
    fn admits_dir(&self, dir: &str, depth: usize) -> bool {
        if dir.len() > MAX_KEY_BYTES || depth >= MAX_COMPONENTS {
            return false;
        }
        if let Some(after) = self.start_after {
            // Every key under `dir` is at least `dir` and starts with it, so
            // a cursor past `dir` that does not start with it is past them all.
            if after > dir && !after.starts_with(dir) {
                return false;
            }
        }
        !self.full()
            || self
                .heap
                .peek()
                .is_some_and(|(largest, _)| dir < largest.as_str())
    }

    fn push(&mut self, key: String, len: u64) {
        self.heap.push((key, len));
        if self.heap.len() > self.cap {
            self.heap.pop();
        }
    }
}

/// Lists up to `max` keys under `root` starting with `prefix`, after
/// `start_after`. A prefix no key can have (an empty or reserved component
/// before its last `/`) lists nothing, as S3 would.
pub(crate) fn page(
    root: &Path,
    prefix: &str,
    start_after: Option<&str>,
    max: usize,
) -> BlobResult<Page> {
    let (dir_part, name_prefix) = match prefix.rsplit_once('/') {
        Some((dir, name)) => (Some(dir), name),
        None => (None, prefix),
    };
    let mut smallest = Smallest {
        // ALLOC: at most max + 1 keys, max capped at MAX_LIST_KEYS by the caller.
        heap: BinaryHeap::with_capacity(max.saturating_add(1)),
        cap: max.saturating_add(1),
        start_after,
    };
    // ALLOC: one directory path per page.
    let mut dir = root.to_path_buf();
    // ALLOC: one key prefix per page, at most MAX_KEY_BYTES plus one.
    let mut base = String::new();
    if let Some(d) = dir_part {
        if super::path::check_fs_key(d).is_err() {
            return Ok(Page {
                // ALLOC: an empty page.
                keys: Vec::new(),
                truncated: false,
            });
        }
        dir.extend(d.split('/'));
        base.push_str(d);
        base.push('/');
    }
    let depth = dir_part.map_or(0, |d| d.split('/').count());
    visit(&dir, &base, depth, name_prefix, &mut smallest)?;
    let mut keys = smallest.heap.into_sorted_vec();
    let truncated = keys.len() > max;
    keys.truncate(max);
    Ok(Page { keys, truncated })
}

/// Adds the keys in `dir`, whose keys start with `base` and have `depth`
/// components before the entry's own, to `smallest`; only entries starting
/// with `name_prefix` at this level. A name is checked against the page
/// before the entry is `stat`ed, as the Java checks it before reading
/// attributes.
fn visit(
    dir: &Path,
    base: &str,
    depth: usize,
    name_prefix: &str,
    smallest: &mut Smallest<'_>,
) -> BlobResult<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotADirectory => return Ok(()),
        Err(e) => return Err(io_error(&e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| io_error(&e))?;
        let file_name = entry.file_name();
        // A name that is not UTF-8 cannot be a key.
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if name == RESERVED_DIR || !name.starts_with(name_prefix) {
            continue;
        }
        // ALLOC: one key per entry, at most MAX_KEY_BYTES past the filters below.
        let mut key =
            String::with_capacity(base.len().saturating_add(name.len()).saturating_add(1));
        key.push_str(base);
        key.push_str(name);
        let kind = entry.file_type().map_err(|e| io_error(&e))?;
        let file_could_enter = key.len() <= MAX_KEY_BYTES && smallest.admits(&key);
        // ALLOC: the entry's key as a directory prefix, at most MAX_KEY_BYTES plus one.
        let mut dir_key = key.clone();
        dir_key.push('/');
        let dir_could_enter = smallest.admits_dir(&dir_key, depth.saturating_add(1));
        if kind.is_file() && !file_could_enter || kind.is_dir() && !dir_could_enter {
            continue;
        }
        if !kind.is_symlink() && !kind.is_file() && !kind.is_dir() {
            continue;
        }
        // A link is followed, as the Java's `Files.readAttributes` does.
        let meta = match fs::metadata(entry.path()) {
            Ok(meta) => meta,
            // Deleted since the directory was read.
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io_error(&e)),
        };
        if meta.is_file() {
            if file_could_enter {
                smallest.push(key, meta.len());
            }
        } else if meta.is_dir() && dir_could_enter {
            visit(
                &entry.path(),
                &dir_key,
                depth.saturating_add(1),
                "",
                smallest,
            )?;
        }
    }
    Ok(())
}
