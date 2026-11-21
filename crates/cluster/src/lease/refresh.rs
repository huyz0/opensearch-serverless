//! Refreshing the view of the fleet: the members index, the lease digest,
//! and a read of each lease whose copy has run out.
//!
//! Ported from `BlobLeaseMembership.java` (`refresh`, `refreshIfOlderThan`,
//! `writesDigest`, `mergeDigest`, `writeDigest`). A refresh of a steady
//! fleet is one read of the index: a lease copy whose own stamp is still in
//! the future cannot have become false, because an expiry only moves
//! forward. One node, the live member with the lowest id, reads every lease
//! each third of a lifetime and writes what it read to the digest; the rest
//! take leases from the digest instead of reading each one. Nothing that
//! decides ownership consults this view: a takeover reads the lease itself
//! ([`super::LivenessOracle`]), so being wrong here costs a retry, never a
//! shard.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use oss_blob::SwapOutcome;
use oss_blob::register::ABSENT_GENERATION;
use oss_format::{MAX_LEASE_BYTES, NodeLease, decode_digest, encode_digest, java_order};

use super::membership::{Membership, index_bytes};
use super::{DEAD_LEASE_PRUNE_TTLS, LeaseConfig, MembershipDelta, lock, same_member};
use crate::error::ClusterError;

/// What refreshes keep between them, and their buffers.
pub(crate) struct View {
    /// Each member's lease as last read, while its stamp is in the future.
    last_read: HashMap<String, NodeLease>,
    /// When the last refresh ran, by the lease clock.
    refreshed_at: Option<i64>,
    /// The index's generation at the last refresh.
    index_generation: u64,
    /// When this node last read the digest, or wrote it.
    digest_read_at: Option<i64>,
    digest_written_at: Option<i64>,
    /// The live leases the last digest named.
    digest_ids: Vec<String>,
    lease: Vec<u8>,
    write: Vec<u8>,
    index: Vec<u8>,
    digest: Vec<u8>,
}

impl View {
    pub(super) fn new(config: &LeaseConfig) -> Self {
        let digest = config
            .max_members
            .saturating_mul(MAX_LEASE_BYTES.saturating_add(4))
            .saturating_add(13);
        Self {
            // ALLOC: one entry a member, at most max_members.
            last_read: HashMap::new(),
            refreshed_at: None,
            index_generation: ABSENT_GENERATION,
            digest_read_at: None,
            digest_written_at: None,
            // ALLOC: one id a member, at most max_members.
            digest_ids: Vec::new(),
            // ALLOC: the refresh's buffers, once a node, bounded by max_members.
            lease: vec![0; MAX_LEASE_BYTES],
            // ALLOC: as above.
            write: vec![0; MAX_LEASE_BYTES],
            // ALLOC: as above.
            index: vec![0; index_bytes(config.max_members)],
            // ALLOC: as above.
            digest: vec![0; digest],
        }
    }
}

/// What one refresh saw.
#[derive(Debug, Clone)]
pub struct Refreshed {
    /// The live members.
    pub live: Arc<[NodeLease]>,
    /// Who joined and left since the last refresh.
    pub delta: MembershipDelta,
    /// Members whose lease has run out or been revoked and still names
    /// write-log epochs, whether or not this node ever saw them live: the
    /// dead nodes whose shards a successor may still have to take. Read from
    /// the leases the refresh already reads.
    pub dead_with_epochs: Vec<NodeLease>,
}

impl Membership {
    /// Refreshes only if the view is older than `max_age_millis`, by the
    /// lease clock; within that age, a join or a clean leave is still seen
    /// by one read of the index's generation, made at most once a probe
    /// interval of wall-clock time. `None` when the view answered.
    ///
    /// # Errors
    ///
    /// As [`Membership::refresh`].
    pub fn refresh_if_older_than(
        &self,
        max_age_millis: i64,
    ) -> Result<Option<Refreshed>, ClusterError> {
        let now = self.clock.now_millis();
        let (refreshed_at, generation) = {
            let view = lock(&self.view);
            (view.refreshed_at, view.index_generation)
        };
        if refreshed_at.is_none_or(|at| now.saturating_sub(at) >= max_age_millis) {
            return self.refresh().map(Some);
        }
        {
            let mut probed = lock(&self.probed_at);
            if probed.is_some_and(|at| at.elapsed() < self.probe_interval) {
                return Ok(None);
            }
            *probed = Some(Instant::now());
        }
        let probe = {
            let mut view = lock(&self.view);
            self.store.read_register(&self.index_key, &mut view.index)?
        };
        match probe {
            Some(read) if read.generation == generation => Ok(None),
            _ => self.refresh().map(Some),
        }
    }

    /// Reads who is live: the index, the digest when it saves reads, and
    /// each listed lease whose held copy has run out. Prunes a lease the
    /// index lists that is gone, or expired for ten lifetimes and naming no
    /// write-log epoch. An unreadable lease is skipped: it can only make
    /// this view smaller, and nothing about safety depends on it.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Blob`] when the index cannot be read;
    /// [`ClusterError::Format`] for an index that does not parse.
    pub fn refresh(&self) -> Result<Refreshed, ClusterError> {
        let mut view = lock(&self.view);
        let view = &mut *view;
        let now = self.clock.now_millis();
        let index = self.read_index(&mut view.index)?;
        let ids = match &index {
            Some((generation, ids)) => {
                view.index_generation = *generation;
                ids.clone()
            }
            // ALLOC: none.
            None => Vec::new(),
        };
        let writing_digest = self.writes_digest(view, now);
        if !writing_digest {
            self.merge_digest(view, &ids, now);
        }
        // ALLOC: one lease a live member, at most max_members.
        let mut live: Vec<NodeLease> = Vec::new();
        // ALLOC: one entry a lease pruned, at most max_members.
        let mut prunable: Vec<(String, u64)> = Vec::new();
        // ALLOC: the dead members that name epochs, at most max_members.
        let mut dead: Vec<NodeLease> = Vec::new();
        for id in &ids {
            if !writing_digest
                && let Some(known) = view.last_read.get(id)
                && !known.is_expired_at(now)
            {
                live.push(known.clone());
                continue;
            }
            match self.read_member(view, id, now, index.is_some()) {
                Ok(Member::Live(lease)) => {
                    let lease = *lease;
                    // ALLOC: one entry a member, at most max_members.
                    view.last_read.insert(id.clone(), lease.clone());
                    live.push(lease);
                }
                Ok(Member::Prunable(generation)) => {
                    view.last_read.remove(id);
                    // ALLOC: as above.
                    prunable.push((id.clone(), generation));
                }
                Ok(Member::Dead(lease)) => {
                    view.last_read.remove(id);
                    // ALLOC: as above.
                    dead.push(*lease);
                }
                Ok(Member::Kept) | Err(_) => {
                    view.last_read.remove(id);
                }
            }
        }
        if let Some((generation, ids)) = &index
            && !prunable.is_empty()
        {
            self.prune(view, ids, *generation, &prunable);
        }
        view.last_read.retain(|id, _| ids.contains(id));
        view.refreshed_at = Some(now);
        if writing_digest {
            self.write_digest(view, &live, now);
        }
        // ALLOC: the live members, once a refresh.
        let live: Arc<[NodeLease]> = Arc::from(live);
        let previous = std::mem::replace(&mut *lock(&self.observed), Arc::clone(&live));
        let joined = live
            .iter()
            .filter(|l| !previous.iter().any(|p| same_member(p, l)))
            .cloned();
        let left = previous
            .iter()
            .filter(|p| !live.iter().any(|l| same_member(p, l)))
            .cloned();
        let delta = MembershipDelta {
            // ALLOC: the members that joined, at most max_members.
            joined: joined.collect(),
            // ALLOC: the members that left, at most max_members.
            left: left.collect(),
        };
        Ok(Refreshed {
            live,
            delta,
            dead_with_epochs: dead,
        })
    }

    /// Reads one listed member's lease.
    fn read_member(
        &self,
        view: &mut View,
        id: &str,
        now: i64,
        indexed: bool,
    ) -> Result<Member, ClusterError> {
        let key = self.lease_key(id)?;
        let Some((generation, lease)) = self.read_lease(&key, &mut view.lease)? else {
            // Listed and gone: a clean leave whose index swap was lost.
            // Claimed by a marker first, so a renewal racing the prune moves
            // the generation and re-enrols.
            if !indexed {
                return Ok(Member::Kept);
            }
            let marker = NodeLease::new(id, "", "", 0);
            let len = marker.write(&mut view.write)?;
            let value = view.write.get(..len).unwrap_or_default();
            return Ok(match self.store.create_register(&key, value)? {
                SwapOutcome::Applied(generation) => Member::Prunable(generation),
                SwapOutcome::Conflict(_) => Member::Kept,
            });
        };
        if !lease.is_expired_at(now) {
            // ALLOC: one lease a live member read, bounded by MAX_LEASE_BYTES.
            return Ok(Member::Live(Box::new(lease)));
        }
        let dead_for = now.saturating_sub(lease.expires_at_millis);
        let long_dead = dead_for >= DEAD_LEASE_PRUNE_TTLS.saturating_mul(self.config.ttl_millis);
        // Never one that names write-log epochs: it is the only place a
        // successor finds them.
        if !lease.node_log.is_empty() {
            // ALLOC: one lease a dead member that names epochs.
            return Ok(Member::Dead(Box::new(lease)));
        }
        if !indexed || !long_dead {
            return Ok(Member::Kept);
        }
        // The same value swapped, so the generation moves and a renewal
        // racing this prune finds it moved and enrols again.
        let len = lease.write(&mut view.write)?;
        let value = view.write.get(..len).unwrap_or_default();
        Ok(match self.store.swap_register(&key, generation, value)? {
            SwapOutcome::Applied(generation) => Member::Prunable(generation),
            SwapOutcome::Conflict(_) => Member::Kept,
        })
    }

    /// Unlists the claimed leases from the index read at `generation`, and
    /// deletes them where conditional deletes are verified. Failures leave
    /// them unlisted or listed: a retained lease costs storage, never a
    /// recurring read past the next prune.
    fn prune(&self, view: &mut View, ids: &[String], generation: u64, prunable: &[(String, u64)]) {
        let remaining = ids
            .iter()
            .filter(|id| !prunable.iter().any(|(p, _)| p == *id))
            .cloned();
        // ALLOC: the remaining ids, at most max_members.
        let remaining: Vec<String> = remaining.collect();
        let unlisted = matches!(
            self.write_index(&remaining, generation, &mut view.index),
            Ok(SwapOutcome::Applied(_))
        );
        if unlisted && self.config.conditional_deletes {
            for (id, generation) in prunable {
                if let Ok(key) = self.lease_key(id) {
                    let _ = self.store.delete_register(&key, *generation);
                }
            }
        }
    }

    /// Whether this node writes the digest on this refresh: it holds a live
    /// lease, no member live at the last refresh has a lower id, and a third
    /// of a lifetime has passed since it last wrote. Two nodes that both
    /// think so for a moment each write a correct digest.
    fn writes_digest(&self, view: &View, now: i64) -> bool {
        let own = lock(&self.own_lease);
        let Some(own) = own.as_ref() else {
            return false;
        };
        let ttl = self.config.ttl_millis;
        if own.is_expired_at(now)
            || view
                .digest_written_at
                .is_some_and(|at| now.saturating_sub(at) < ttl / 3)
        {
            return false;
        }
        // Live at the last refresh, judged with a lifetime of slack: the copy
        // held may have run out without the node having gone.
        !lock(&self.observed).iter().any(|m| {
            !m.revoked
                && now.saturating_sub(m.expires_at_millis) < ttl
                && java_order(&m.node_id, &own.node_id).is_lt()
        })
    }

    /// Takes from the digest every listed lease later than the copy held.
    /// Read only when a lease the last digest named (or, with none, any
    /// lease) has run out here, and at most once a tenth of a lifetime. An
    /// unreadable digest is no digest.
    fn merge_digest(&self, view: &mut View, ids: &[String], now: i64) {
        let named = if view.digest_ids.is_empty() {
            ids
        } else {
            &view.digest_ids
        };
        let due = named.iter().any(|id| {
            ids.contains(id)
                && view
                    .last_read
                    .get(id)
                    .is_none_or(|held| held.is_expired_at(now))
        });
        let recent = view
            .digest_read_at
            .is_some_and(|at| now.saturating_sub(at) < self.config.ttl_millis / 10);
        if !due || recent {
            return;
        }
        view.digest_read_at = Some(now);
        view.digest_ids.clear();
        let Ok(Some(read)) = self.store.read_register(&self.digest_key, &mut view.digest) else {
            return;
        };
        let Ok(digest) = decode_digest(view.digest.get(..read.len).unwrap_or_default()) else {
            return;
        };
        for lease in digest.leases {
            if !ids.contains(&lease.node_id) || lease.is_expired_at(now) {
                continue;
            }
            // ALLOC: one id a member, at most max_members.
            view.digest_ids.push(lease.node_id.clone());
            let later = view
                .last_read
                .get(&lease.node_id)
                .is_none_or(|held| lease.expires_at_millis > held.expires_at_millis);
            if later {
                // ALLOC: one entry a member, at most max_members.
                view.last_read.insert(lease.node_id.clone(), lease);
            }
        }
    }

    /// Writes what this refresh read, against whatever digest is stored;
    /// losing the swap means another node wrote one.
    fn write_digest(&self, view: &mut View, live: &[NodeLease], now: i64) {
        view.digest_written_at = Some(now);
        view.digest_read_at = Some(now);
        // ALLOC: one id a member, at most max_members.
        view.digest_ids = live.iter().map(|l| l.node_id.clone()).collect();
        // The stored digest is read for its generation first, then the
        // buffer is reused for the new one.
        let current = match self.store.read_register(&self.digest_key, &mut view.digest) {
            Ok(read) => read.map(|r| r.generation),
            Err(_) => return,
        };
        let Ok(len) = encode_digest(now, live, &mut view.digest) else {
            return;
        };
        let value = view.digest.get(..len).unwrap_or_default();
        let _ = match current {
            None => self.store.create_register(&self.digest_key, value),
            Some(generation) => self
                .store
                .swap_register(&self.digest_key, generation, value),
        };
    }
}

/// What a read of one listed member found.
enum Member {
    Live(Box<NodeLease>),
    /// Claimed for pruning at this generation.
    Prunable(u64),
    /// Expired or revoked, naming write-log epochs: kept, and reported.
    Dead(Box<NodeLease>),
    /// Expired but kept, or not claimed.
    Kept,
}
