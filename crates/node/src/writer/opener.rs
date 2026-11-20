//! Opening a shard on its first write: win its head and open it under the
//! head's term.
//!
//! Ported from the open path of `ShardReconciler` in huyz0/OpenSearch
//! `feature/serverlessshell`, for a shard no writer has published yet.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_cluster::heads::{Acquisition, ShardHeadStore};
use oss_format::ShardRef;
use oss_meta::DescriptorStore;
use oss_writer::bulk::IndexInfo;
use oss_writer::mapping::Mapping;

use std::sync::Mutex;

use super::{ShardSlot, Takeover, WriterRole, lock};
use oss_writer::takeover::restore;

/// What a role asks when a write names a shard it does not hold.
pub trait ShardOpener: Send + Sync {
    /// Opens `shard` of `index` on `role`.
    ///
    /// # Errors
    ///
    /// Why the shard cannot be held here, as a message.
    fn open(&self, role: &WriterRole, index: &IndexInfo, shard: u32) -> Result<(), String>;
}

/// Opens shards whose head this process wins and which hold no commit yet.
pub struct HeadOpener {
    heads: Arc<ShardHeadStore>,
    descriptors: Arc<DescriptorStore>,
    store: Arc<dyn BlobStore>,
    base: String,
    node_id: String,
    ephemeral_id: String,
    buffer_bytes: usize,
    takeover: Option<Takeover>,
}

impl HeadOpener {
    /// An opener for process `ephemeral_id` of node `node_id`, reading
    /// registers and descriptors through buffers of `buffer_bytes`.
    pub fn new(
        heads: Arc<ShardHeadStore>,
        descriptors: Arc<DescriptorStore>,
        store: Arc<dyn BlobStore>,
        base: &str,
        node_id: &str,
        ephemeral_id: &str,
        buffer_bytes: usize,
    ) -> Self {
        Self {
            heads,
            descriptors,
            store,
            // ALLOC: three names, once a node.
            base: base.to_owned(),
            node_id: node_id.to_owned(),
            ephemeral_id: ephemeral_id.to_owned(),
            buffer_bytes,
            takeover: None,
        }
    }

    /// Lets this opener take over shards that have a history: restore the
    /// published commit and replay the head's writers. Without it such a
    /// shard is refused.
    #[must_use]
    pub fn with_takeover(mut self, takeover: Takeover) -> Self {
        self.takeover = Some(takeover);
        self
    }
}

const TAKEOVER: &str = "takeover of a shard with a history is not supported by this node yet";

impl ShardOpener for HeadOpener {
    fn open(&self, role: &WriterRole, index: &IndexInfo, shard: u32) -> Result<(), String> {
        let won = self
            .heads
            .acquire(
                &index.name,
                shard,
                &self.node_id,
                &self.ephemeral_id,
                Some(&index.uuid),
            )
            .map_err(|e| e.to_string())?;
        let head = match won {
            Acquisition::Won { head, .. } => head,
            Acquisition::HeldByAnother(other) => {
                return Err(format!(
                    "[{}][{shard}] is owned by node [{}]",
                    index.name,
                    other.owner_node_id.as_deref().unwrap_or("unknown")
                ));
            }
        };
        let at = ShardRef {
            index: &index.name,
            uuid: &index.uuid,
            shard,
        };
        let key = at.manifest_key(&self.base).map_err(|e| e.to_string())?;
        // ALLOC: one buffer a shard opened, buffer_bytes.
        let mut buf = vec![0u8; self.buffer_bytes];
        let committed = self
            .store
            .read_register(&key, &mut buf)
            .map_err(|e| e.to_string())?
            .is_some();
        let history = committed || !head.writers.is_empty();
        if history && self.takeover.is_none() {
            return Err(format!("[{}][{shard}]: {TAKEOVER}", index.name));
        }
        let record = self
            .descriptors
            .get(&index.name, &mut buf)
            .map_err(|e| e.to_string())?
            .filter(|record| record.uuid == index.uuid)
            .ok_or_else(|| {
                // ALLOC: error path; one message.
                format!("[{}] is no longer the index this write named", index.name)
            })?;
        let mapping = Mapping::from_descriptor(record.mapping.as_deref())
            .map_err(|e| e.failed_to_parse().to_string())?;
        let Some(takeover) = self.takeover.as_ref().filter(|_| history) else {
            return role.open_shard(index, shard, head.term, mapping);
        };
        role.open_taken(index, shard, &head, mapping, takeover, |dir| {
            // ALLOC: one buffer a restore, buffer_bytes.
            let mut buf = vec![0u8; self.buffer_bytes];
            restore(self.store.as_ref(), &self.base, &at, dir, &mut buf)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }
}

impl WriterRole {
    /// Asks `opener` to open a shard a write names and this node does not
    /// hold.
    #[must_use]
    pub fn with_opener(mut self, opener: Arc<dyn ShardOpener>) -> Self {
        self.opener = Some(opener);
        self
    }

    /// The held shard, opened first through the opener when it is not held.
    pub(super) fn slot_or_open(
        &self,
        index: &IndexInfo,
        shard: u32,
    ) -> Result<Arc<Mutex<ShardSlot>>, String> {
        if let Some(slot) = self.slot(&index.uuid, shard) {
            return Ok(slot);
        }
        let not_open = || {
            // ALLOC: error path; one message.
            format!(
                "cannot index into [{}][{shard}]: not open on this node",
                index.name
            )
        };
        let opener = self.opener.as_ref().ok_or_else(not_open)?;
        let _opening = lock(&self.opening);
        // Another group may have opened it while this one waited.
        if let Some(slot) = self.slot(&index.uuid, shard) {
            return Ok(slot);
        }
        opener.open(self, index, shard)?;
        self.slot(&index.uuid, shard).ok_or_else(not_open)
    }
}
