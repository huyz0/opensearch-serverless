//! What earlier sweeps saw unreferenced: the grace.

use std::collections::{BTreeMap, HashMap};

/// What the sweeps of each shard saw unreferenced, and when each blob was
/// first seen so, in the sweep's clock.
///
/// Held by the caller between sweeps and keyed by the shard's storage path,
/// which names its index's uuid, so a replaced index starts empty. A node
/// that has just taken a shard has an empty memory: a new owner cannot know
/// how long a blob has been unreferenced, and assuming "long enough" is the
/// one assumption that loses data. Forgetting only delays a deletion.
#[derive(Debug, Default)]
pub struct Memory {
    // ALLOC: one entry a shard being watched, and one key a blob seen
    // unreferenced and not yet deleted; an empty shard holds no entry.
    shards: HashMap<String, BTreeMap<String, i64>>,
}

impl Memory {
    /// An empty memory.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many blobs of the shard at `path` are being watched.
    pub fn watching(&self, path: &str) -> usize {
        self.shards.get(path).map_or(0, BTreeMap::len)
    }

    /// How many shards have a blob being watched.
    pub fn shards(&self) -> usize {
        self.shards.len()
    }

    /// Forgets what is remembered of the shard at `path`: the node let it go,
    /// and a later owner starts empty.
    pub fn forget(&mut self, path: &str) {
        self.shards.remove(path);
    }

    pub(super) fn seen(&self, path: &str) -> Option<&BTreeMap<String, i64>> {
        self.shards.get(path)
    }

    /// Replaces what is remembered of `path`; nothing, for no entry.
    pub(super) fn set(&mut self, path: &str, seen: BTreeMap<String, i64>) {
        if seen.is_empty() {
            self.shards.remove(path);
        } else {
            // ALLOC: one key a shard first watched.
            self.shards.insert(path.to_owned(), seen);
        }
    }
}
