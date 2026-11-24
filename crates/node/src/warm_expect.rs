//! Which pages of a shard a joining reader accepts from a peer.
//!
//! The joiner opens the shard first. Its open builds each file's page-cache
//! key from its own namespace, file name, stored length and blob key, so the
//! keys and page geometry of the open commit are what the joiner itself would
//! read. A page is accepted only when it names one of those keys, a page
//! inside that file, with the length the file stores there; nothing a peer
//! says widens the set. Pinned names and decoded files are not warmed.

use std::collections::HashMap;

use oss_reader::ShardFiles;

/// The accepted plain pages of one open shard, by cache key.
#[derive(Debug, Default)]
pub struct Expected {
    page_size: u64,
    /// Each file's cache key and the bytes it stores.
    files: HashMap<String, u64>,
}

impl Expected {
    /// What the open commit `files` of the shard at `namespace`
    /// (`{base}/{index}/{shard}/t={term}`) keys its pages by, at `page_size`;
    /// `pinned` says which names the open reads whole and the cache holds
    /// apart.
    pub fn new(
        namespace: &str,
        files: &ShardFiles,
        page_size: usize,
        pinned: &dyn Fn(&str) -> bool,
    ) -> Self {
        // ALLOC: one entry per file of the commit, which the manifest bounds.
        let mut keyed = HashMap::new();
        for name in files.names().filter(|n| !pinned(n)) {
            let Some(range) = files.get(name) else {
                continue;
            };
            let key = format!(
                "{namespace}\u{0}{name}\u{0}{len}\u{0}p\u{0}v{versions}:{version}",
                len = range.length,
                versions = range.key.len(),
                version = range.key,
            );
            keyed.insert(key, range.length);
        }
        Self {
            page_size: u64::try_from(page_size).unwrap_or(u64::MAX),
            files: keyed,
        }
    }

    /// How many bytes page `page` of the file with cache key `key` stores,
    /// or `None` when no such page is accepted.
    pub fn page_len(&self, key: &str, page: u64) -> Option<u64> {
        let stored = *self.files.get(key)?;
        let start = page.checked_mul(self.page_size)?;
        if start >= stored {
            return None;
        }
        Some(stored.saturating_sub(start).min(self.page_size))
    }

    /// Whether a peer's page of `bytes` at `key` and `page` is accepted.
    pub fn accepts(&self, key: &str, page: u64, bytes: usize) -> bool {
        self.page_len(key, page)
            .is_some_and(|len| u64::try_from(bytes).is_ok_and(|b| b == len))
    }

    /// How many files are accepted.
    pub fn files(&self) -> usize {
        self.files.len()
    }
}

#[cfg(test)]
mod tests;
