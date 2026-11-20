//! Stopping a head from naming the logs a take replayed.

use std::sync::Arc;

use oss_blob::{BlobStore, SwapOutcome};
use oss_cluster::heads::shard_head_key;
use oss_format::{MAX_SHARD_HEAD_BYTES, ShardHead};

use super::AdoptError;

/// How settling a head came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// The head named none of the writers: nothing was written.
    NothingToClear,
    /// The writers are no longer named.
    Cleared,
    /// The head is another term's or another owner's: not this node's to
    /// write, and nothing was.
    Lost,
    /// The head is still this node's, but other writes to it won the swap
    /// twice running: nothing was cleared, and a later settle may.
    Contended,
}

/// A deployment's heads, for clearing the writers a take replayed.
pub struct HeadWriters {
    store: Arc<dyn BlobStore>,
    base: String,
}

impl HeadWriters {
    /// The heads under `base` (empty, or ending in `/`).
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Self {
        Self {
            store,
            // ALLOC: the base, once a store.
            base: base.to_owned(),
        }
    }

    /// Stops the head of `won` naming `replayed`, while it is still `won`'s
    /// term and owner: one GET and one compare-and-swap, and again from a
    /// fresh read if another write to the head (a lease renewal) won the
    /// swap, once.
    ///
    /// # Errors
    ///
    /// The store failing, or a head that does not parse.
    pub fn settle(&self, won: &ShardHead, replayed: &[String]) -> Result<Settled, AdoptError> {
        let key = shard_head_key(&self.base, &won.index, won.shard);
        let (Some(node), Some(ephemeral)) = (
            won.owner_node_id.as_deref(),
            won.owner_ephemeral_id.as_deref(),
        ) else {
            return Ok(Settled::Lost);
        };
        for _ in 0..2 {
            // ALLOC: one head's bytes a read, at most MAX_SHARD_HEAD_BYTES.
            let mut buf = vec![0u8; MAX_SHARD_HEAD_BYTES];
            let Some(read) = self
                .store
                .read_register(&key, &mut buf)
                .map_err(AdoptError::Head)?
            else {
                return Ok(Settled::Lost);
            };
            let current = ShardHead::parse(buf.get(..read.len).unwrap_or_default())
                .map_err(AdoptError::HeadFormat)?;
            if current.term != won.term || !current.owned_by(node, ephemeral) {
                return Ok(Settled::Lost);
            }
            let mut next = current.clone();
            next.writers.retain(|w| !replayed.contains(w));
            if next.writers == current.writers {
                return Ok(Settled::NothingToClear);
            }
            let bytes = next.to_bytes().map_err(AdoptError::HeadFormat)?;
            if let SwapOutcome::Applied(_) = self
                .store
                .swap_register(&key, read.generation, &bytes)
                .map_err(AdoptError::Head)?
            {
                return Ok(Settled::Cleared);
            }
        }
        Ok(Settled::Contended)
    }
}
