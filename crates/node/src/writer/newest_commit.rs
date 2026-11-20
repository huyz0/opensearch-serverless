//! The commit each held shard last published, remembered for readers that ask.

use super::{MAX_NEWEST_BYTES, WriterRole, lock};
use std::sync::Arc;

impl WriterRole {
    /// Remembers `manifest`, the commit a publish of the shard produced,
    /// for readers that ask. False, and nothing remembered, when the shard
    /// is not held or the manifest is over [`MAX_NEWEST_BYTES`].
    pub fn staged(&self, index_uuid: &str, shard: u32, manifest: &[u8]) -> bool {
        if manifest.len() > MAX_NEWEST_BYTES {
            return false;
        }
        // ALLOC: one key a call.
        let key = (index_uuid.to_owned(), shard);
        // Held across the insert, so a release between the check and the
        // insert cannot leave a manifest for a shard no longer held.
        let shards = lock(&self.shards);
        if !shards.contains_key(&key) {
            return false;
        }
        // ALLOC: one manifest a held shard, at most MAX_NEWEST_BYTES.
        lock(&self.newest).insert(key, Arc::from(manifest));
        true
    }

    /// The commit the shard last published, if it is held and has.
    pub fn newest_commit(&self, index_uuid: &str, shard: u32) -> Option<Arc<[u8]>> {
        // ALLOC: one key a lookup.
        let key = (index_uuid.to_owned(), shard);
        let shards = lock(&self.shards);
        if !shards.contains_key(&key) {
            return None;
        }
        lock(&self.newest).get(&key).cloned()
    }
}
