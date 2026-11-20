//! Reading and removing the heads of one index incarnation, for a delete.
//!
//! Ported from `ShardHeadStore.readAllOf`, `deleteIfUnchanged`,
//! `releaseAllOf` and `deleteAllOf`. Every method is one request a shard of
//! the index, by derived key: never a listing.

use std::sync::Arc;

use oss_blob::{BlobStore, SwapOutcome};
use oss_format::{MAX_SHARD_HEAD_BYTES, ShardHead};

use super::shard_head_key;
use crate::error::ClusterError;

/// One shard's head as read before a delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadAt {
    /// The shard.
    pub shard: u32,
    /// The head register's generation when read.
    pub generation: u64,
    /// Whether the head names an owner: a writer that may yet publish.
    pub owned: bool,
}

/// A deployment's shard head registers, for removing an index's.
pub struct HeadRegisters {
    store: Arc<dyn BlobStore>,
    base: String,
}

impl HeadRegisters {
    /// The heads under `base` (empty, or ending in `/`).
    ///
    /// # Errors
    ///
    /// [`ClusterError::Format`] for a base that is not empty and does not
    /// end in `/`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Result<Self, ClusterError> {
        oss_format::cluster_config_key(base)?;
        Ok(Self {
            store,
            // ALLOC: the base, once a store.
            base: base.to_owned(),
        })
    }

    /// The head of `shard` with its generation, `None` if absent.
    fn read(&self, index: &str, shard: u32) -> Result<Option<(u64, ShardHead)>, ClusterError> {
        // ALLOC: one head's bytes a read, at most MAX_SHARD_HEAD_BYTES.
        let mut buf = vec![0u8; MAX_SHARD_HEAD_BYTES];
        let key = shard_head_key(&self.base, index, shard);
        let Some(read) = self.store.read_register(&key, &mut buf)? else {
            return Ok(None);
        };
        let head = ShardHead::parse(buf.get(..read.len).unwrap_or_default())?;
        Ok(Some((read.generation, head)))
    }

    /// The heads of incarnation `uuid` of `index` below `shards` that exist.
    /// A head naming no uuid is the index's: the descriptor being deleted
    /// names this one, and a head is keyed by name.
    ///
    /// # Errors
    ///
    /// The store failing, or a head that does not parse.
    pub fn read_all_of(
        &self,
        index: &str,
        shards: u32,
        uuid: &str,
    ) -> Result<Vec<HeadAt>, ClusterError> {
        // ALLOC: one entry a head, at most the shard count.
        let mut found = Vec::new();
        for shard in 0..shards {
            if let Some((generation, head)) = self.read(index, shard)?
                && head.index_uuid.as_deref().is_none_or(|u| u == uuid)
            {
                found.push(HeadAt {
                    shard,
                    generation,
                    owned: head.owner_node_id.is_some(),
                });
            }
        }
        Ok(found)
    }

    /// Removes each of `heads` only if it still holds the generation read: a
    /// head moved since is a new incarnation's or a writer's.
    ///
    /// # Errors
    ///
    /// The store failing.
    pub fn delete_if_unchanged(&self, index: &str, heads: &[HeadAt]) -> Result<(), ClusterError> {
        for head in heads {
            let key = shard_head_key(&self.base, index, head.shard);
            self.store.delete_register(&key, head.generation)?;
        }
        Ok(())
    }

    /// Clears the owner of every head of incarnation `uuid`, where the store
    /// cannot delete conditionally. The term stays, so a successor's take
    /// still moves it forward; a head that moved under the swap is left.
    ///
    /// # Errors
    ///
    /// The store failing, or a head that does not parse.
    pub fn release_all_of(&self, index: &str, shards: u32, uuid: &str) -> Result<(), ClusterError> {
        for shard in 0..shards {
            let Some((generation, mut head)) = self.read(index, shard)? else {
                continue;
            };
            if head.index_uuid.as_deref().is_some_and(|u| u != uuid) {
                continue;
            }
            head.owner_node_id = None;
            head.owner_ephemeral_id = None;
            head.lease_expires_at_millis = 0;
            let key = shard_head_key(&self.base, index, shard);
            let _: SwapOutcome = self
                .store
                .swap_register(&key, generation, &head.to_bytes()?)?;
        }
        Ok(())
    }

    /// Removes every head naming incarnation `uuid` of `index`, conditionally,
    /// and answers how many there were: one found after a delete was
    /// re-created by a writer that missed it. A head of another incarnation,
    /// or naming none, stays: by then the name may be a new index's, and a
    /// head with no uuid cannot be told apart from its head.
    ///
    /// # Errors
    ///
    /// The store failing, or a head that does not parse.
    pub fn delete_all_of(
        &self,
        index: &str,
        shards: u32,
        uuid: &str,
    ) -> Result<usize, ClusterError> {
        let mut removed = 0usize;
        for shard in 0..shards {
            let Some((generation, head)) = self.read(index, shard)? else {
                continue;
            };
            if head.index_uuid.as_deref() != Some(uuid) {
                continue;
            }
            let key = shard_head_key(&self.base, index, shard);
            if self.store.delete_register(&key, generation)? {
                removed = removed.saturating_add(1);
            }
        }
        Ok(removed)
    }
}
