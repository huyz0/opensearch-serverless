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

use super::hibernate::Slept;
use super::{ShardSlot, Takeover, WriterRole, lock, publish_age_of};
use oss_writer::takeover::{restore, restore_value};

/// Why a shard cannot be opened here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRefused {
    /// Why, as a message.
    pub reason: String,
    /// The live node whose head holds the shard, when that is why.
    pub holder: Option<String>,
}

impl std::fmt::Display for OpenRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl From<String> for OpenRefused {
    fn from(reason: String) -> Self {
        Self {
            reason,
            holder: None,
        }
    }
}

/// What a role asks when a write names a shard it does not hold.
pub trait ShardOpener: Send + Sync {
    /// Opens `shard` of `index` on `role`.
    ///
    /// # Errors
    ///
    /// Why the shard cannot be held here, with the holder when another live
    /// node holds it.
    fn open(&self, role: &WriterRole, index: &IndexInfo, shard: u32) -> Result<(), OpenRefused>;

    /// Gives back the head of a shard this node closed at `term`, when it
    /// still names this process; whether it did. An opener with no head to
    /// give back answers false.
    fn give_back(&self, _index: &str, _uuid: &str, _shard: u32, _term: i64) -> bool {
        false
    }
}

/// Reads a bucket member's latest commit and its version: the bucket and the
/// index uuid in, `None` before the member's first commit.
pub type MemberCommitRead =
    Arc<dyn Fn(&str, &str) -> Result<Option<(Vec<u8>, u64)>, String> + Send + Sync>;

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
    member_commit: Option<MemberCommitRead>,
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
            member_commit: None,
        }
    }

    /// Reads the commit of a bucket member from its bucket's manifest, where
    /// a shard of its own would have a register.
    #[must_use]
    pub fn with_member_commits(mut self, read: MemberCommitRead) -> Self {
        self.member_commit = Some(read);
        self
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
    fn open(&self, role: &WriterRole, index: &IndexInfo, shard: u32) -> Result<(), OpenRefused> {
        self.open_shard(role, index, shard)
    }

    fn give_back(&self, index: &str, uuid: &str, shard: u32, term: i64) -> bool {
        // A failed release leaves the head this node's: the next write wakes
        // or takes it as it would have.
        self.heads
            .release(index, shard, &self.node_id, &self.ephemeral_id, term, uuid)
            .unwrap_or(false)
    }
}

impl HeadOpener {
    fn open_shard(
        &self,
        role: &WriterRole,
        index: &IndexInfo,
        shard: u32,
    ) -> Result<(), OpenRefused> {
        let slept = role.take_asleep(&index.uuid, shard);
        let ours = self.still_ours(index, shard, slept);
        let woke = ours.is_some();
        let won = match ours {
            Some(head) => Acquisition::Won {
                head,
                taken_from: None,
            },
            None => self
                .heads
                .acquire(
                    &index.name,
                    shard,
                    &self.node_id,
                    &self.ephemeral_id,
                    Some(&index.uuid),
                )
                .map_err(|e| OpenRefused::from(e.to_string()))?,
        };
        let head = match won {
            Acquisition::Won { head, .. } => head,
            Acquisition::HeldByAnother(other) => {
                return Err(OpenRefused {
                    reason: format!(
                        "[{}][{shard}] is owned by node [{}]",
                        index.name,
                        other.owner_node_id.as_deref().unwrap_or("unknown")
                    ),
                    holder: other.owner_node_id.filter(|n| *n != self.node_id),
                });
            }
        };
        let at = ShardRef {
            index: &index.name,
            uuid: &index.uuid,
            shard,
        };
        // ALLOC: one buffer a shard opened, buffer_bytes.
        let mut buf = vec![0u8; self.buffer_bytes];
        let record = self
            .descriptors
            .get(&index.name, &mut buf)
            .map_err(|e| e.to_string())?
            .filter(|record| record.uuid == index.uuid)
            .ok_or_else(|| {
                // ALLOC: error path; one message.
                format!("[{}] is no longer the index this write named", index.name)
            })?;
        // A member of a bucket has its commit in the bucket's manifest, and no register.
        let member = match (record.bucket.as_deref(), &self.member_commit) {
            (Some(bucket), Some(read)) => Some(read(bucket, &index.uuid)?),
            _ => None,
        };
        let bucketed = member.is_some();
        let committed_own = if bucketed {
            None
        } else {
            let key = at.manifest_key(&self.base).map_err(|e| e.to_string())?;
            // ALLOC: one buffer a shard opened, buffer_bytes.
            let mut reg = vec![0u8; self.buffer_bytes];
            self.store
                .read_register(&key, &mut reg)
                .map_err(|e| e.to_string())?
                .map(|_| ())
        };
        let member = member.flatten();
        let committed = member.is_some() || committed_own.is_some();
        let history = committed || !head.writers.is_empty();
        if history && self.takeover.is_none() {
            return Err(format!("[{}][{shard}]: {TAKEOVER}", index.name).into());
        }
        let mapping = Mapping::from_descriptor(record.mapping.as_deref())
            .map_err(|e| e.failed_to_parse().to_string())?;
        let age = publish_age_of(record.settings);
        match self.takeover.as_ref().filter(|_| history) {
            None => role.open_shard(index, shard, head.term, mapping)?,
            Some(takeover) => role.open_taken(index, shard, &head, mapping, takeover, |dir| {
                // ALLOC: one buffer a restore, buffer_bytes.
                let mut buf = vec![0u8; self.buffer_bytes];
                match (bucketed, &member) {
                    (true, Some((value, version))) => restore_value(
                        self.store.as_ref(),
                        &self.base,
                        &at,
                        value,
                        *version,
                        dir,
                        &mut buf,
                    )
                    .map(|_| ()),
                    // A member with no commit yet restores nothing.
                    (true, None) => Ok(()),
                    (false, _) => {
                        restore(self.store.as_ref(), &self.base, &at, dir, &mut buf).map(|_| ())
                    }
                }
                .map_err(|e| e.to_string())
            })?,
        }
        role.set_publish_age(&index.uuid, shard, age);
        if woke {
            role.woke(&index.uuid, shard);
        }
        Ok(())
    }

    /// The head of a shard that slept here, when it still names this process
    /// at the term it slept at: one read, and no new term to take.
    fn still_ours(
        &self,
        index: &IndexInfo,
        shard: u32,
        slept: Option<Slept>,
    ) -> Option<oss_format::ShardHead> {
        let slept = slept?;
        let head = self.heads.read(&index.name, shard).ok().flatten()?;
        (head.term == slept.term
            && head.owner_node_id.as_deref() == Some(self.node_id.as_str())
            && head.owner_ephemeral_id.as_deref() == Some(self.ephemeral_id.as_str())
            && head.index_uuid.as_deref().is_none_or(|u| u == index.uuid))
        .then_some(head)
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
    ) -> Result<Arc<Mutex<ShardSlot>>, OpenRefused> {
        if let Some(slot) = self.slot(&index.uuid, shard) {
            return Ok(slot);
        }
        let not_open = || {
            OpenRefused::from(format!(
                // ALLOC: error path; one message.
                "cannot index into [{}][{shard}]: not open on this node",
                index.name
            ))
        };
        let opener = self.opener.as_ref().ok_or_else(not_open)?;
        let _opening = lock(&self.opening);
        // Another group may have opened it while this one waited.
        if let Some(slot) = self.slot(&index.uuid, shard) {
            return Ok(slot);
        }
        self.admit_open(index, shard)?;
        // A shard woken and put to sleep again before it is looked up is
        // woken again; a threshold near zero is the only way to see it.
        for _ in 0..3 {
            opener.open(self, index, shard)?;
            if let Some(slot) = self.slot(&index.uuid, shard) {
                return Ok(slot);
            }
        }
        Err(not_open())
    }
}

impl WriterRole {
    /// Holds `shard` of `index`, opening it through the opener when it is
    /// not held: the take a write would make, made without one.
    ///
    /// # Errors
    ///
    /// Why the shard cannot be held here.
    pub fn hold(&self, index: &IndexInfo, shard: u32) -> Result<(), OpenRefused> {
        self.slot_or_open(index, shard).map(|_| ())
    }
}
