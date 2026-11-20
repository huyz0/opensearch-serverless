//! [`ShardHeads`] over the shard head registers.

use std::sync::Arc;

use oss_blob::{BlobError, BlobStore};
use oss_cluster::ClusterError;
use oss_cluster::heads::{HeadAt as Head, HeadRegisters};
use oss_meta::{HeadAt, MetaError, ShardHeads};

/// The heads a delete reads and removes, one request a shard.
pub struct NodeHeads {
    registers: HeadRegisters,
}

impl NodeHeads {
    /// The heads under `base`.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base that is not empty and does not end in `/`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Result<Self, MetaError> {
        Ok(Self {
            registers: HeadRegisters::new(store, base).map_err(meta)?,
        })
    }
}

fn meta(e: ClusterError) -> MetaError {
    match e {
        ClusterError::Blob(b) => MetaError::Blob(b),
        ClusterError::Format(f) => MetaError::Format(f),
        _ => MetaError::Blob(BlobError::Protocol("a shard head could not be handled")),
    }
}

impl ShardHeads for NodeHeads {
    fn read_all_of(&self, index: &str, shards: u32, uuid: &str) -> Result<Vec<HeadAt>, MetaError> {
        Ok(self
            .registers
            .read_all_of(index, shards, uuid)
            .map_err(meta)?
            .into_iter()
            .map(|h| HeadAt {
                shard: h.shard,
                generation: h.generation,
                owned: h.owned,
            })
            // ALLOC: one entry a head, at most the shard count.
            .collect())
    }

    fn delete_if_unchanged(&self, index: &str, heads: &[HeadAt]) -> Result<(), MetaError> {
        // ALLOC: one entry a head, at most the shard count.
        let heads: Vec<Head> = heads
            .iter()
            .map(|h| Head {
                shard: h.shard,
                generation: h.generation,
                owned: h.owned,
            })
            .collect();
        self.registers
            .delete_if_unchanged(index, &heads)
            .map_err(meta)
    }

    fn release_all_of(&self, index: &str, shards: u32, uuid: &str) -> Result<(), MetaError> {
        self.registers
            .release_all_of(index, shards, uuid)
            .map_err(meta)
    }

    fn delete_all_of(&self, index: &str, shards: u32, uuid: &str) -> Result<usize, MetaError> {
        self.registers
            .delete_all_of(index, shards, uuid)
            .map_err(meta)
    }
}
