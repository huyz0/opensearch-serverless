//! Reading and taking a shard's head.

use std::sync::Arc;

use oss_blob::{BlobStore, SwapOutcome};
use oss_format::{MAX_SHARD_HEAD_BYTES, ShardHead};

use super::shard_head_key;
use crate::error::ClusterError;
use crate::lease::{LeaseClock, LivenessOracle};

/// What a take came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acquisition {
    /// This process owns the shard now.
    Won {
        /// The head as written.
        head: ShardHead,
        /// The node the shard was taken from, if another owned it.
        taken_from: Option<String>,
    },
    /// Another owner is live, or won the race: its head.
    HeldByAnother(ShardHead),
}

/// A deployment's shard heads.
pub struct ShardHeadStore {
    store: Arc<dyn BlobStore>,
    base: String,
    clock: Arc<dyn LeaseClock>,
    lease_ttl_millis: i64,
    oracle: Arc<LivenessOracle>,
}

impl ShardHeadStore {
    /// The heads under `base`, an acquisition stamping its claim
    /// `lease_ttl_millis` ahead, judging an owner's liveness by `oracle`.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Format`] for a base that is not empty and does not
    /// end in `/`.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        clock: Arc<dyn LeaseClock>,
        lease_ttl_millis: i64,
        oracle: Arc<LivenessOracle>,
    ) -> Result<Self, ClusterError> {
        // The same check every register key makes.
        oss_format::cluster_config_key(base)?;
        Ok(Self {
            store,
            // ALLOC: the base, once a store.
            base: base.to_owned(),
            clock,
            lease_ttl_millis,
            oracle,
        })
    }

    fn read_raw(&self, key: &str) -> Result<Option<(u64, ShardHead)>, ClusterError> {
        // ALLOC: one head's bytes a read, at most MAX_SHARD_HEAD_BYTES.
        let mut buf = vec![0u8; MAX_SHARD_HEAD_BYTES];
        let Some(read) = self.store.read_register(key, &mut buf)? else {
            return Ok(None);
        };
        let head = ShardHead::parse(buf.get(..read.len).unwrap_or_default())?;
        Ok(Some((read.generation, head)))
    }

    /// The head of shard `shard` of `index`, or `None` if the shard was
    /// never activated: one GET.
    ///
    /// # Errors
    ///
    /// The store failing, or a head that does not parse.
    pub fn read(&self, index: &str, shard: u32) -> Result<Option<ShardHead>, ClusterError> {
        let key = shard_head_key(&self.base, index, shard);
        Ok(self.read_raw(&key)?.map(|(_, head)| head))
    }

    /// Tries to take the shard for this process, bumping its term.
    ///
    /// An absent head is created at term 1. An unowned head, one whose
    /// owner is not live, and one left by another incarnation of the index
    /// (same name, other uuid) are taken at the next term, naming the
    /// previous owner among the writers whose logs the new owner reads. A
    /// live owner refuses, and so does a lost race, with the winner's head.
    ///
    /// # Errors
    ///
    /// The store failing, or a head that does not parse.
    pub fn acquire(
        &self,
        index: &str,
        shard: u32,
        node_id: &str,
        ephemeral_id: &str,
        index_uuid: Option<&str>,
    ) -> Result<Acquisition, ClusterError> {
        let key = shard_head_key(&self.base, index, shard);
        let now = self.clock.now_millis();
        let claim = now.saturating_add(self.lease_ttl_millis);
        let owned = |term: i64| ShardHead {
            // ALLOC: one head a take.
            index: index.to_owned(),
            shard,
            term,
            // ALLOC: the ids of one head a take.
            owner_node_id: Some(node_id.to_owned()),
            // ALLOC: as above.
            owner_ephemeral_id: Some(ephemeral_id.to_owned()),
            lease_expires_at_millis: claim,
            // ALLOC: as above.
            index_uuid: index_uuid.map(str::to_owned),
            // ALLOC: none until a writer is named.
            writers: Vec::new(),
        };
        let Some((generation, current)) = self.read_raw(&key)? else {
            let head = owned(1);
            return match self.store.create_register(&key, &head.to_bytes()?)? {
                SwapOutcome::Applied(_) => Ok(Acquisition::Won {
                    head,
                    taken_from: None,
                }),
                SwapOutcome::Conflict(_) => self.winner(&key),
            };
        };
        let other_incarnation = match (index_uuid, current.index_uuid.as_deref()) {
            (Some(ours), Some(theirs)) => ours != theirs,
            _ => false,
        };
        // A dead owner's lease is revoked before its shard is taken, so a
        // clock beyond the skew budget cannot leave it serving on. Not for
        // another incarnation, nor for this node's own id.
        if !other_incarnation
            && let Some(owner) = current.owner_node_id.as_deref()
            && owner != node_id
            && (current.is_held_at(now)
                || !self
                    .oracle
                    .revoke(owner, current.owner_ephemeral_id.as_deref()))
        {
            return Ok(Acquisition::HeldByAnother(current));
        }
        let taken_from = if other_incarnation {
            None
        } else {
            current.owner_node_id.clone()
        };
        let mut next = owned(current.term.saturating_add(1));
        if !other_incarnation {
            next.writers = current.writers.clone();
            next = next.with_writer(current.owner_node_id.as_deref());
        }
        match self
            .store
            .swap_register(&key, generation, &next.to_bytes()?)?
        {
            SwapOutcome::Applied(_) => Ok(Acquisition::Won {
                head: next,
                taken_from,
            }),
            SwapOutcome::Conflict(_) => self.winner(&key),
        }
    }

    /// The head a lost race left: the winner's.
    fn winner(&self, key: &str) -> Result<Acquisition, ClusterError> {
        match self.read_raw(key)? {
            Some((_, head)) => Ok(Acquisition::HeldByAnother(head)),
            None => Err(ClusterError::Heads(
                "a shard head vanished after a lost race",
            )),
        }
    }
}
