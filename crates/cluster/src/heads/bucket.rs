//! A bucket's members are owned through one head.
//!
//! Ported from `BucketHeads` and the member paths of `ShardHeadStore`
//! (huyz0/OpenSearch `feature/serverlessshell`). The bucket head is the
//! shard head named `#bucket#<bucket id>`, shard 0: a name no index can have.
//! It carries the bucket's identity and no incarnation, since its members'
//! incarnations are their descriptors' business. Joining a bucket this node
//! already holds writes nothing and does not move the term, so every member
//! open here serves at the bucket's one term.

use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::SwapOutcome;
use oss_format::{ShardHead, bucket_manifest_key};

use super::shard_head_key;
use super::store::{Acquisition, ShardHeadStore};
use crate::error::ClusterError;

/// A member shard: the bucket that owns it, and the member index's uuid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    bucket_id: String,
    index_uuid: String,
}

impl Member {
    /// A member of `bucket_id`, the index of `index_uuid`.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Format`] for a bucket id that is not 1 to 64 of
    /// `a-z`, `0-9`, `-` and `_`.
    pub fn new(bucket_id: &str, index_uuid: &str) -> Result<Self, ClusterError> {
        bucket_manifest_key("", bucket_id)?;
        Ok(Self {
            // ALLOC: one member's two ids.
            bucket_id: bucket_id.to_owned(),
            // ALLOC: as above.
            index_uuid: index_uuid.to_owned(),
        })
    }

    /// The bucket that owns the shard.
    #[must_use]
    pub fn bucket_id(&self) -> &str {
        &self.bucket_id
    }

    /// The member index's uuid.
    #[must_use]
    pub fn index_uuid(&self) -> &str {
        &self.index_uuid
    }

    /// The bucket head's own key.
    fn key(&self, base: &str) -> String {
        shard_head_key(base, &bucket_head_index(&self.bucket_id), 0)
    }

    /// A stored bucket head as the member's: its index name and shard, its
    /// uuid, and the bucket's term, owner, lease and writers.
    fn shown(&self, index: &str, shard: u32, stored: &ShardHead) -> ShardHead {
        ShardHead {
            // ALLOC: one head shown a take.
            index: index.to_owned(),
            shard,
            term: stored.term,
            owner_node_id: stored.owner_node_id.clone(),
            owner_ephemeral_id: stored.owner_ephemeral_id.clone(),
            lease_expires_at_millis: stored.lease_expires_at_millis,
            // ALLOC: as above.
            index_uuid: Some(self.index_uuid.clone()),
            writers: stored.writers.clone(),
        }
    }

    /// A bucket head to store: the bucket's identity, never a member's.
    fn stored(&self, term: i64, node_id: &str, ephemeral_id: &str, expires: i64) -> ShardHead {
        ShardHead {
            // ALLOC: one head a take.
            index: bucket_head_index(&self.bucket_id),
            shard: 0,
            term,
            // ALLOC: the ids of one head a take.
            owner_node_id: Some(node_id.to_owned()),
            // ALLOC: as above.
            owner_ephemeral_id: Some(ephemeral_id.to_owned()),
            lease_expires_at_millis: expires,
            index_uuid: None,
            // ALLOC: none until a writer is named.
            writers: Vec::new(),
        }
    }
}

/// The index name a bucket's head is recorded under.
#[must_use]
pub fn bucket_head_index(bucket_id: &str) -> String {
    // ALLOC: one name a head operation.
    format!("#bucket#{bucket_id}")
}

/// Which shards belong to buckets.
///
/// It must answer the same on every node, which is why an implementation
/// reads the index's descriptor, and why buckets are turned on for a whole
/// fleet before the first one is made.
pub trait BucketHeads: Send + Sync {
    /// The bucket a shard of one incarnation of `index` belongs to: `None`
    /// for a shard owned through its own head, or one of another
    /// incarnation than `index_uuid` (`None` when the caller does not know
    /// it).
    ///
    /// # Errors
    ///
    /// The index's descriptor cannot be read.
    fn bucket_of(
        &self,
        index: &str,
        shard: u32,
        index_uuid: Option<&str>,
    ) -> Result<Option<Member>, ClusterError>;
}

impl ShardHeadStore {
    /// Routes the shards `heads` names members of to their bucket's head.
    #[must_use]
    pub fn with_bucket_heads(mut self, heads: Arc<dyn BucketHeads>) -> Self {
        self.buckets = Some(heads);
        self
    }

    /// The bucket a shard belongs to, when buckets are routed.
    pub(super) fn member_of(
        &self,
        index: &str,
        shard: u32,
        index_uuid: Option<&str>,
    ) -> Result<Option<Member>, ClusterError> {
        match &self.buckets {
            Some(heads) => heads.bucket_of(index, shard, index_uuid),
            None => Ok(None),
        }
    }

    /// A member's head, shown as its own: one GET.
    pub(super) fn read_member(
        &self,
        member: &Member,
        index: &str,
        shard: u32,
    ) -> Result<Option<ShardHead>, ClusterError> {
        let key = member.key(&self.base);
        Ok(self
            .read_raw(&key)?
            .map(|(_, head)| member.shown(index, shard, &head)))
    }

    /// Takes a bucket member, one take of a bucket at a time on this node.
    pub(super) fn acquire_member(
        &self,
        member: &Member,
        index: &str,
        shard: u32,
        node_id: &str,
        ephemeral_id: &str,
    ) -> Result<Acquisition, ClusterError> {
        // One take at a time: members open in parallel, and each racing to
        // create or swap the one head is a billed request that loses. The
        // first takes it and the rest join it free.
        let turn = {
            let mut turns = self.turns.lock().unwrap_or_else(PoisonError::into_inner);
            // ALLOC: one turn per bucket this node takes, bounded by the
            // buckets it holds.
            Arc::clone(turns.entry(member.bucket_id.clone()).or_default())
        };
        let _turn = turn.lock().unwrap_or_else(PoisonError::into_inner);
        self.acquire_member_in_turn(member, index, shard, node_id, ephemeral_id)
    }

    fn acquire_member_in_turn(
        &self,
        member: &Member,
        index: &str,
        shard: u32,
        node_id: &str,
        ephemeral_id: &str,
    ) -> Result<Acquisition, ClusterError> {
        let key = member.key(&self.base);
        let now = self.clock.now_millis();
        let expires = now.saturating_add(self.lease_ttl_millis);
        let Some((generation, current)) = self.read_raw(&key)? else {
            let head = member.stored(1, node_id, ephemeral_id, expires);
            return match self.store.create_register(&key, &head.to_bytes()?)? {
                SwapOutcome::Applied(_) => Ok(Acquisition::Won {
                    head: member.shown(index, shard, &head),
                    taken_from: None,
                }),
                SwapOutcome::Conflict(_) => self.member_winner(member, index, shard),
            };
        };
        let ours = current.owner_node_id.as_deref() == Some(node_id)
            && current
                .owner_ephemeral_id
                .as_deref()
                .is_none_or(|e| e == ephemeral_id);
        if ours {
            return Ok(Acquisition::Won {
                head: member.shown(index, shard, &current),
                taken_from: None,
            });
        }
        if let Some(owner) = current.owner_node_id.as_deref()
            && (current.is_held_at(now)
                || !self
                    .oracle
                    .revoke(owner, current.owner_ephemeral_id.as_deref()))
        {
            return Ok(Acquisition::HeldByAnother(
                member.shown(index, shard, &current),
            ));
        }
        let mut next = member.stored(
            current.term.saturating_add(1),
            node_id,
            ephemeral_id,
            expires,
        );
        next.writers = current.writers.clone();
        next = next.with_writer(current.owner_node_id.as_deref());
        match self
            .store
            .swap_register(&key, generation, &next.to_bytes()?)?
        {
            SwapOutcome::Applied(_) => Ok(Acquisition::Won {
                head: member.shown(index, shard, &next),
                taken_from: current.owner_node_id.clone(),
            }),
            SwapOutcome::Conflict(_) => self.member_winner(member, index, shard),
        }
    }

    /// The head a lost race left, shown as the member's.
    fn member_winner(
        &self,
        member: &Member,
        index: &str,
        shard: u32,
    ) -> Result<Acquisition, ClusterError> {
        match self.read_member(member, index, shard)? {
            Some(head) => Ok(Acquisition::HeldByAnother(head)),
            None => Err(ClusterError::Heads(
                "a bucket head vanished after a lost race",
            )),
        }
    }
}

/// A map from a mutex per bucket.
pub(super) type Turns = Mutex<std::collections::HashMap<String, Arc<Mutex<()>>>>;
