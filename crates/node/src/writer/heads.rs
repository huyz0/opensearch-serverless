//! Admission of a shard's records to an epoch of the node log: the shard's
//! head must still name this process as its owner, at the term the
//! records are written under.
//!
//! The Java checks the same head before a shard's first records enter an
//! epoch (`NodeLogCommitter`'s admission over `ShardHeadStore`): a
//! successor moves the head and then fences every epoch in this node's
//! lease, so either this check sees the move or the fence closes the epoch.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use oss_cluster::heads::ShardHeadStore;
use oss_format::ShardKey;
use oss_writer::log::{Refusal, ShardAdmission};

/// The names of the indexes this node holds, by uuid: a log record names
/// its shard by uuid, and a head is keyed by name.
#[derive(Default)]
pub struct ShardNames {
    names: Mutex<HashMap<String, String>>,
}

impl ShardNames {
    /// Remembers that index `uuid` is called `name`.
    pub fn insert(&self, uuid: &str, name: &str) {
        // ALLOC: one entry an index held.
        self.names
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(uuid.to_owned(), name.to_owned());
    }

    fn name_of(&self, uuid: &str) -> Option<String> {
        self.names
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(uuid)
            .cloned()
    }
}

/// The committer's [`ShardAdmission`] over shard heads: one read.
pub struct HeadAdmission {
    heads: Arc<ShardHeadStore>,
    names: Arc<ShardNames>,
    node_id: String,
    ephemeral_id: String,
}

impl HeadAdmission {
    /// Admission for process `ephemeral_id` of node `node_id`.
    pub fn new(
        heads: Arc<ShardHeadStore>,
        names: Arc<ShardNames>,
        node_id: &str,
        ephemeral_id: &str,
    ) -> Self {
        Self {
            heads,
            names,
            // ALLOC: the ids, once a node.
            node_id: node_id.to_owned(),
            ephemeral_id: ephemeral_id.to_owned(),
        }
    }
}

fn refuse(why: String) -> Refusal {
    Arc::new(io::Error::other(why))
}

impl ShardAdmission for HeadAdmission {
    fn admit(&self, shard: &ShardKey, term: i64, _epoch: u64) -> Result<(), Refusal> {
        let uuid = shard.index_uuid.as_ref();
        let Some(name) = self.names.name_of(uuid) else {
            return Err(refuse(format!("index [{uuid}] is not held here")));
        };
        let head = self
            .heads
            .read(&name, shard.shard)
            .map_err(|e| refuse(e.to_string()))?;
        match head {
            Some(h) if h.term == term && h.owned_by(&self.node_id, &self.ephemeral_id) => Ok(()),
            Some(h) => Err(refuse(format!(
                "[{name}][{}] is owned at term {} by {:?}, not this node at term {term}",
                shard.shard, h.term, h.owner_node_id
            ))),
            None => Err(refuse(format!("[{name}][{}] has no head", shard.shard))),
        }
    }
}
