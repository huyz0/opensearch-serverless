//! Which files an open reads whole, and the ranges that fetch them.

use std::collections::BTreeMap;

use super::OpenPrefetch;
use crate::shard_files::ShardFiles;

/// One file inside a [`Range`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// The Lucene file name.
    pub name: String,
    /// Where the file starts inside the range.
    pub at: u64,
    /// The file's length.
    pub len: u64,
}

/// One ranged GET: bytes `start..start + len` of blob `key`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    /// The blob.
    pub key: String,
    /// First byte.
    pub start: u64,
    /// Bytes to read.
    pub len: u64,
    /// The files the range holds, in blob order.
    pub members: Vec<Member>,
}

/// The bytes `ranges` read, gaps included: what an open holds while its
/// prefetch runs.
pub fn planned_bytes(ranges: &[Range]) -> u64 {
    ranges
        .iter()
        .fold(0u64, |total, range| total.saturating_add(range.len))
}

/// The ranges that fetch what the engine reads whole when it opens `files`:
/// files whose name `pinned` accepts, and files of at most `page_size`
/// bytes. A file that would pass `max_bytes` of files is not picked, and a
/// range that would pass `max_bytes` read, gaps included, is dropped; files
/// of one blob at most `gap_bytes` apart share a range.
pub fn plan(
    files: &ShardFiles,
    pinned: &dyn Fn(&str) -> bool,
    page_size: u64,
    config: &OpenPrefetch,
) -> Vec<Range> {
    // ALLOC: one entry a picked file, at most the manifest's file limit.
    let mut by_blob: BTreeMap<&str, Vec<(u64, u64, &str)>> = BTreeMap::new();
    let mut picked = 0u64;
    for name in files.names() {
        let Some(file) = files.get(name) else {
            continue;
        };
        if file.length == 0 || !(pinned(name) || file.length <= page_size) {
            continue;
        }
        let Some(total) = picked.checked_add(file.length) else {
            break;
        };
        if total > config.max_bytes {
            continue;
        }
        picked = total;
        by_blob
            .entry(file.key.as_str())
            .or_default()
            .push((file.offset, file.length, name));
    }
    // ALLOC: one range a blob at most, bounded by the picked files.
    let mut ranges = Vec::new();
    for (key, mut members) in by_blob {
        members.sort_unstable();
        let mut open: Option<Range> = None;
        for (offset, len, name) in members {
            let end = offset.saturating_add(len);
            if let Some(range) = open.as_mut() {
                let range_end = range.start.saturating_add(range.len);
                if offset <= range_end.saturating_add(config.gap_bytes) {
                    range.len = end.max(range_end).saturating_sub(range.start);
                    range
                        .members
                        .push(member(name, offset.saturating_sub(range.start), len));
                    continue;
                }
            }
            ranges.extend(open.take());
            open = Some(Range {
                // ALLOC: one key a range.
                key: key.to_owned(),
                start: offset,
                len,
                // ALLOC: one member a picked file.
                members: vec![member(name, 0, len)],
            });
        }
        ranges.extend(open);
    }
    // The bytes read, the gaps between files included, stay within the
    // limit: a range that would pass it is left to the engine.
    let mut read = 0u64;
    ranges.retain(|r| {
        let Some(total) = read.checked_add(r.len).filter(|t| *t <= config.max_bytes) else {
            return false;
        };
        read = total;
        true
    });
    ranges
}

fn member(name: &str, at: u64, len: u64) -> Member {
    Member {
        // ALLOC: one name a picked file.
        name: name.to_owned(),
        at,
        len,
    }
}
