//! What an index's delete asks of its shards: their heads, the pins that
//! keep their bytes, and the purge of those bytes.
//!
//! Shard heads and the snapshot and point-in-time pins are ported with the
//! writer and snapshot work, and the lifecycle reaches them through
//! [`ShardHeads`] and [`ShardPins`]. The crate ships no implementation that
//! answers "no heads" or "no pins": one would purge bytes a Java writer is
//! still publishing or a Java snapshot still names, so nothing can wire a
//! delete until the real ones exist (ADR 0012). The purge is the Java's
//! `IndexLifecycle.purgeShardData`.

use oss_blob::{BlobStore, MAX_LIST_KEYS};
use oss_format::ShardRef;

use crate::error::MetaError;

/// One shard's head as read before a delete: the generation it is removed
/// at, and whether a node owned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadAt {
    /// The shard.
    pub shard: u32,
    /// The head register's generation when read.
    pub generation: u64,
    /// Whether the head named an owner: a writer that may yet publish.
    pub owned: bool,
}

/// An index's shard heads, as the Java's `ShardHeadStore` serves a delete.
/// Every method is bounded by the shard count, never by a listing.
pub trait ShardHeads: Send + Sync {
    /// The heads of incarnation `uuid` of `index` that exist, one read a
    /// shard (`readAllOf`).
    ///
    /// # Errors
    ///
    /// [`MetaError`] when a head cannot be read.
    fn read_all_of(&self, index: &str, shards: u32, uuid: &str) -> Result<Vec<HeadAt>, MetaError>;

    /// Removes each of `heads` only if it still holds the generation read
    /// (`deleteIfUnchanged`): a head moved since is a new incarnation's.
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the store fails.
    fn delete_if_unchanged(&self, index: &str, heads: &[HeadAt]) -> Result<(), MetaError>;

    /// Releases every head of incarnation `uuid` of `index`, where the store
    /// cannot delete conditionally (`releaseAllOf`).
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the store fails.
    fn release_all_of(&self, index: &str, shards: u32, uuid: &str) -> Result<(), MetaError>;

    /// Removes every head still naming incarnation `uuid` of `index`,
    /// conditionally, and answers how many there were (`deleteAllOf`): a head
    /// found after the delete was re-created by a writer that missed it.
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the store fails.
    fn delete_all_of(&self, index: &str, shards: u32, uuid: &str) -> Result<usize, MetaError>;
}

/// What keeps a deleted index's shard bytes: a live shallow snapshot or a
/// point in time naming them directly (the Java's `pinnedBy` and
/// `pinnedByView`).
pub trait ShardPins: Send + Sync {
    /// The shards of incarnation `uuid` of `index`, below `shards`, that a
    /// live snapshot or point in time pins, in any order. Read once a purge.
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the pins cannot be read; then nothing is purged.
    fn pinned(&self, index: &str, uuid: &str, shards: u32) -> Result<Vec<u32>, MetaError>;
}

/// What a purge left: shards a pin kept, and shards the store failed to
/// clear, whose bytes are storage nothing serves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Purged {
    /// Shards left because a pin names them.
    pub pinned: u32,
    /// Shards the store failed to clear.
    pub failed: u32,
}

/// Removes every blob under each unpinned shard's storage path of
/// incarnation `uuid` of `index`: its segments, manifest and logs. Best
/// effort a shard, as the Java's: a failure leaves storage, which costs
/// money, and is counted rather than failing a delete that already
/// happened.
///
/// # Errors
///
/// [`MetaError`] when the pins cannot be read, before anything is removed.
pub(crate) fn purge(
    store: &dyn BlobStore,
    base: &str,
    pins: &dyn ShardPins,
    index: &str,
    uuid: &str,
    shards: u32,
) -> Result<Purged, MetaError> {
    let pinned = pins.pinned(index, uuid, shards)?;
    let mut purged = Purged::default();
    for shard in 0..shards {
        if pinned.contains(&shard) {
            purged.pinned = purged.pinned.saturating_add(1);
            continue;
        }
        let path = ShardRef { index, uuid, shard }.base_path(base)?;
        if clear(store, &path).is_err() {
            purged.failed = purged.failed.saturating_add(1);
        }
    }
    Ok(purged)
}

/// Deletes every key under `path`, a page at a time.
fn clear(store: &dyn BlobStore, path: &str) -> Result<(), MetaError> {
    // ALLOC: one LIST's keys, at most MAX_LIST_KEYS of MAX_KEY_BYTES.
    let mut keys: Vec<String> = Vec::with_capacity(MAX_LIST_KEYS);
    let mut after: Option<String> = None;
    loop {
        keys.clear();
        // LIST: a deleted shard's storage, found by its derived prefix; one
        // a shard of a deleted index and one more a thousand blobs, so the
        // LISTs scale with the bytes being removed, never with time.
        let page = store.list_prefix(path, after.as_deref(), MAX_LIST_KEYS, &mut |key, _| {
            // ALLOC: one key a listed blob, bounded by the page.
            keys.push(key.to_owned());
        })?;
        for key in &keys {
            store.delete(key)?;
        }
        if !page.truncated {
            return Ok(());
        }
        let Some(last) = keys.pop() else {
            return Ok(());
        };
        after = Some(last);
    }
}
