//! A node's own lease: renewing it, letting it go, revoking a dead peer's,
//! and the no-I/O checks the write path fences on.
//!
//! Ported from `BlobLeaseMembership.java` (`renew`, `revokedUnderneath`,
//! `revoke`, `enrol`, `release`, `read`, `enrolledIds`, `forgetNodeLog`,
//! `selfLeaseValidAt`, `selfLeaseLapsedAt`, `current`).

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobStore, SwapOutcome};
use oss_format::{
    MAX_LEASE_BYTES, MAX_NODE_ID_BYTES, NodeLease, decode_members, encode_members, java_order,
    lease_digest_key, lease_key, members_index_key,
};

use super::refresh::View;
use super::{
    ENROL_ATTEMPTS, GENERATION_PROBE_INTERVAL, LeaseClock, LeaseConfig, SKEW_MARGIN_DIVISOR, lock,
    renewed_until,
};
use crate::error::ClusterError;

/// What only this node's renewals and release touch, held across their
/// requests: two renewals at once would each expect the other's generation.
struct Own {
    /// The generation this node last wrote its lease at.
    generation: u64,
    /// Whether this node is on the members index, as far as it knows.
    enrolled: bool,
    /// Whether the next enrolment must swap the index even if it lists this
    /// node: the lease register moved underneath, maybe by a prune.
    force_swap: bool,
    read: Vec<u8>,
    write: Vec<u8>,
    index: Vec<u8>,
}

/// One node's view of the fleet's leases, and its own lease.
pub struct Membership {
    pub(super) store: Arc<dyn BlobStore>,
    pub(super) clock: Arc<dyn LeaseClock>,
    pub(super) config: LeaseConfig,
    pub(super) base: String,
    pub(super) index_key: String,
    pub(super) digest_key: String,
    skew_margin: AtomicI64,
    own: Mutex<Own>,
    /// The lease this node last renewed, in its own view while unexpired.
    pub(super) own_lease: Mutex<Option<NodeLease>>,
    /// The expiry this node last stamped, 0 before its first renewal.
    own_expires: AtomicI64,
    pub(super) view: Mutex<View>,
    pub(super) observed: Mutex<Arc<[NodeLease]>>,
    pub(super) probed_at: Mutex<Option<Instant>>,
    pub(super) probe_interval: Duration,
}

/// The bytes an index naming `members` ids may take.
pub(super) fn index_bytes(members: usize) -> usize {
    members.saturating_mul(MAX_NODE_ID_BYTES.saturating_add(1))
}

impl Membership {
    /// Membership over the leases under `base` (empty, or ending in `/`) of
    /// `store`.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Format`] for a base that is not empty and does not
    /// end in `/`, or a lifetime that is not positive.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        clock: Arc<dyn LeaseClock>,
        config: LeaseConfig,
    ) -> Result<Self, ClusterError> {
        if config.ttl_millis <= 0 {
            return Err(ClusterError::Format(oss_format::FormatError::Corrupt {
                what: "lease config",
                reason: "a lease lifetime is positive",
            }));
        }
        Ok(Self {
            store,
            clock,
            config,
            // ALLOC: the base, once a node.
            base: base.to_owned(),
            index_key: members_index_key(base)?,
            digest_key: lease_digest_key(base)?,
            skew_margin: AtomicI64::new(config.ttl_millis / SKEW_MARGIN_DIVISOR),
            own: Mutex::new(Own {
                generation: ABSENT_GENERATION,
                enrolled: false,
                force_swap: false,
                // ALLOC: two lease buffers and an index buffer, once a node.
                read: vec![0; MAX_LEASE_BYTES],
                // ALLOC: as above.
                write: vec![0; MAX_LEASE_BYTES],
                // ALLOC: as above, bounded by max_members.
                index: vec![0; index_bytes(config.max_members)],
            }),
            own_lease: Mutex::new(None),
            own_expires: AtomicI64::new(0),
            view: Mutex::new(View::new(&config)),
            observed: Mutex::new(Arc::from([])),
            probed_at: Mutex::new(None),
            probe_interval: GENERATION_PROBE_INTERVAL,
        })
    }

    /// The same, answering every young-snapshot call with a probe of the
    /// index at most once `interval` (one second by default).
    #[must_use]
    pub fn with_probe_interval(mut self, interval: Duration) -> Self {
        self.probe_interval = interval;
        self
    }

    /// The lease lifetime this stamps.
    pub fn ttl_millis(&self) -> i64 {
        self.config.ttl_millis
    }

    /// How much earlier than its stamped expiry this node stops trusting its
    /// own lease: `ttl / 32` unless set.
    pub fn skew_margin_millis(&self) -> i64 {
        self.skew_margin.load(Ordering::Relaxed)
    }

    /// Sets the skew margin, for an operator with worse clocks.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Format`] for a margin outside `[0, ttl)`.
    pub fn set_skew_margin_millis(&self, margin: i64) -> Result<(), ClusterError> {
        if !(0..self.config.ttl_millis).contains(&margin) {
            return Err(ClusterError::Format(oss_format::FormatError::Corrupt {
                what: "lease config",
                reason: "the skew margin is within [0, ttl)",
            }));
        }
        self.skew_margin.store(margin, Ordering::Relaxed);
        Ok(())
    }

    pub(super) fn lease_key(&self, node_id: &str) -> Result<String, ClusterError> {
        Ok(lease_key(&self.base, node_id)?)
    }

    /// Reads the lease at `key` through `buf`: its generation and lease.
    pub(super) fn read_lease(
        &self,
        key: &str,
        buf: &mut [u8],
    ) -> Result<Option<(u64, NodeLease)>, ClusterError> {
        let Some(read) = self.store.read_register(key, buf)? else {
            return Ok(None);
        };
        let lease = NodeLease::parse(buf.get(..read.len).unwrap_or_default())?;
        Ok(Some((read.generation, lease)))
    }

    /// Reads the members index through `buf`: its generation and ids.
    pub(super) fn read_index(
        &self,
        buf: &mut [u8],
    ) -> Result<Option<(u64, Vec<String>)>, ClusterError> {
        let Some(read) = self.store.read_register(&self.index_key, buf)? else {
            return Ok(None);
        };
        let ids = decode_members(buf.get(..read.len).unwrap_or_default())?;
        Ok(Some((read.generation, ids)))
    }

    /// Writes the members index naming `ids` through `buf`: swapped from
    /// `expected`, or created when that is [`ABSENT_GENERATION`].
    pub(super) fn write_index(
        &self,
        ids: &[String],
        expected: u64,
        buf: &mut [u8],
    ) -> Result<SwapOutcome, ClusterError> {
        let len = encode_members(ids, buf)?;
        let value = buf.get(..len).unwrap_or_default();
        Ok(if expected == ABSENT_GENERATION {
            self.store.create_register(&self.index_key, value)?
        } else {
            self.store.swap_register(&self.index_key, expected, value)?
        })
    }

    /// Writes or renews this node's lease, stamping its expiry at
    /// `now + ttl`, and returns the lease as written. A swap from the
    /// generation this node last wrote: one request once enrolled.
    ///
    /// This does not check whether the lease had lapsed: a node whose lease
    /// lapsed has lost every shard it held, and must let go of them before
    /// it renews ([`Membership::self_lapsed_at`] is the question to ask
    /// first).
    ///
    /// # Errors
    ///
    /// [`ClusterError::Revoked`] when a successor revoked this process's
    /// lease, which is not written over: the next renewal starts afresh.
    /// [`ClusterError::Duplicate`] when another live process at another
    /// address holds the id. [`ClusterError::Contended`] when the register
    /// moved again under the retry. [`ClusterError::Blob`] when the store
    /// fails, and [`ClusterError::Format`] for a lease or index that does
    /// not fit its bound.
    pub fn renew(&self, lease: &NodeLease) -> Result<NodeLease, ClusterError> {
        let mut own = lock(&self.own);
        let own = &mut *own;
        let now = self.clock.now_millis();
        let renewed = renewed_until(lease, now.saturating_add(self.config.ttl_millis));
        let key = self.lease_key(&lease.node_id)?;
        let len = renewed.write(&mut own.write)?;
        let value = own.write.get(..len).unwrap_or_default();
        // A first write is a create, one request; a swap from an absent
        // generation would read first.
        let mut outcome = if own.generation == ABSENT_GENERATION {
            self.store.create_register(&key, value)?
        } else {
            self.store.swap_register(&key, own.generation, value)?
        };
        if let SwapOutcome::Conflict(_) = outcome {
            if own.generation != ABSENT_GENERATION
                && self.revoked_underneath(&key, lease, &mut own.read)
            {
                own.generation = ABSENT_GENERATION;
                own.enrolled = false;
                return Err(ClusterError::Revoked {
                    // ALLOC: the id, once a revocation.
                    node_id: lease.node_id.clone(),
                });
            }
            let actual = match self.store.read_register(&key, &mut own.read)? {
                Some(read) => {
                    // An unreadable lease is not a duplicate: retry against
                    // whatever generation is stored.
                    let stored = own.read.get(..read.len).unwrap_or_default();
                    if let Ok(other) = NodeLease::parse(stored)
                        && other.ephemeral_id != lease.ephemeral_id
                        && other.expires_at_millis > now
                        && other.address != lease.address
                    {
                        return Err(ClusterError::Duplicate {
                            node_id: other.node_id,
                            other_ephemeral_id: other.ephemeral_id,
                            other_address: other.address,
                        });
                    }
                    read.generation
                }
                None => ABSENT_GENERATION,
            };
            outcome = self.store.swap_register(&key, actual, value)?;
            if let SwapOutcome::Conflict(_) = outcome {
                return Err(ClusterError::Contended);
            }
            // The register moved underneath, most often because a refresh
            // pruned it while this node was away: whatever it remembered
            // about being enrolled is no evidence now.
            own.enrolled = false;
            own.force_swap = true;
        }
        if let SwapOutcome::Applied(generation) = outcome {
            own.generation = generation;
        }
        self.own_expires
            .store(renewed.expires_at_millis, Ordering::Release);
        // ALLOC: one lease copy a renewal, bounded by MAX_LEASE_BYTES.
        *lock(&self.own_lease) = Some(renewed.clone());
        if !own.enrolled && self.enrol(&lease.node_id, own)? {
            own.enrolled = true;
            own.force_swap = false;
        }
        Ok(renewed)
    }

    /// Whether the register holds a revoked copy of this very process's
    /// lease. A lease removed by a release or a prune is not, nor is an
    /// unreadable one.
    fn revoked_underneath(&self, key: &str, lease: &NodeLease, buf: &mut [u8]) -> bool {
        matches!(
            self.read_lease(key, buf),
            Ok(Some((_, stored))) if stored.revoked && stored.ephemeral_id == lease.ephemeral_id
        )
    }

    /// Adds `node_id` to the members index; true once it is there. A few
    /// attempts with a short random pause between them, since a co-started
    /// fleet contends on this one register; one that loses them all tries
    /// again on its next renewal.
    fn enrol(&self, node_id: &str, own: &mut Own) -> Result<bool, ClusterError> {
        for attempt in 0..ENROL_ATTEMPTS {
            if attempt > 0 {
                self.clock.pause(jitter(attempt));
            }
            let (generation, mut ids) = self
                .read_index(&mut own.index)?
                // ALLOC: none until an id is added.
                .unwrap_or((ABSENT_GENERATION, Vec::new()));
            let at = ids.binary_search_by(|id| java_order(id, node_id));
            match at {
                Ok(_) if !own.force_swap && generation != ABSENT_GENERATION => return Ok(true),
                Ok(_) => {}
                // ALLOC: one id, within the index's bound.
                Err(at) => ids.insert(at, node_id.to_owned()),
            }
            if let SwapOutcome::Applied(_) = self.write_index(&ids, generation, &mut own.index)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether this node is on the members index, as far as it knows: true
    /// once an enrolment succeeded since the register last moved under it.
    pub fn is_enrolled(&self) -> bool {
        lock(&self.own).enrolled
    }

    /// Whether this node's own lease is still valid by its own clock, with
    /// no I/O: true before the first renewal, then until its expiry less
    /// the skew margin.
    pub fn self_valid_at(&self, now_millis: i64) -> bool {
        let expiry = self.own_expires.load(Ordering::Acquire);
        expiry == 0 || now_millis < expiry.saturating_sub(self.skew_margin_millis())
    }

    /// Whether this node once held a lease and no longer does, by its own
    /// clock and margin: the question to ask before renewing.
    pub fn self_lapsed_at(&self, now_millis: i64) -> bool {
        let expiry = self.own_expires.load(Ordering::Acquire);
        expiry != 0 && now_millis >= expiry.saturating_sub(self.skew_margin_millis())
    }

    /// The expiry this node last stamped on its own lease, 0 before the
    /// first renewal.
    pub fn own_expires_at_millis(&self) -> i64 {
        self.own_expires.load(Ordering::Acquire)
    }

    /// Removes this node's lease on a clean shutdown, so peers need not
    /// wait out its lifetime: the lease swapped to expire at 0, the node
    /// taken off the index, and the register deleted where conditional
    /// deletes are verified and it names no write-log epoch. A lease of
    /// another process under this id is left alone.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Blob`] when the store fails; [`ClusterError::Format`]
    /// for a lease or index that does not parse.
    pub fn release(&self, node_id: &str) -> Result<(), ClusterError> {
        let mut own = lock(&self.own);
        let own = &mut *own;
        let key = self.lease_key(node_id)?;
        let index = self.read_index(&mut own.index)?;
        if let Some((generation, stored)) = self.read_lease(&key, &mut own.read)? {
            let mine = lock(&self.own_lease).as_ref().is_none_or(|own_lease| {
                own_lease.node_id != node_id || own_lease.ephemeral_id == stored.ephemeral_id
            });
            if !mine {
                return Ok(());
            }
            let released = renewed_until(&stored, 0);
            let len = released.write(&mut own.write)?;
            let value = own.write.get(..len).unwrap_or_default();
            let SwapOutcome::Applied(released_at) =
                self.store.swap_register(&key, generation, value)?
            else {
                return Ok(());
            };
            if let Some((index_generation, mut ids)) = index {
                ids.retain(|id| id != node_id);
                self.write_index(&ids, index_generation, &mut own.index)?;
            }
            if self.config.conditional_deletes && stored.node_log.is_empty() {
                self.store.delete_register(&key, released_at)?;
            }
        }
        own.generation = ABSENT_GENERATION;
        own.enrolled = false;
        let mut own_lease = lock(&self.own_lease);
        if own_lease.as_ref().is_some_and(|l| l.node_id == node_id) {
            *own_lease = None;
            self.own_expires.store(0, Ordering::Release);
        }
        Ok(())
    }

    /// Every node on the members index, live or not: one read.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Blob`] when the store fails; [`ClusterError::Format`]
    /// for an index that does not parse.
    pub fn enrolled_ids(&self) -> Result<Vec<String>, ClusterError> {
        let mut own = lock(&self.own);
        Ok(self
            .read_index(&mut own.index)?
            .map(|(_, ids)| ids)
            // ALLOC: none.
            .unwrap_or_default())
    }

    /// The members as last refreshed, and this node itself while its own
    /// lease is unexpired, even before a refresh has seen it.
    pub fn current(&self) -> Arc<[NodeLease]> {
        let seen = Arc::clone(&lock(&self.observed));
        let own = lock(&self.own_lease);
        match own.as_ref() {
            Some(own)
                if !own.is_expired_at(self.clock.now_millis())
                    && !seen.iter().any(|m| m.node_id == own.node_id) =>
            {
                // ALLOC: the members and this node, once a call that adds it.
                seen.iter().cloned().chain([own.clone()]).collect()
            }
            _ => seen,
        }
    }
}

/// A few to a few tens of milliseconds, growing with the attempt, so
/// contending nodes do not retry in step.
fn jitter(attempt: u32) -> Duration {
    use std::hash::BuildHasher;
    let upper = 200u64.min(10u64.saturating_mul(1u64 << attempt.min(4)));
    let random = std::collections::hash_map::RandomState::new().hash_one(attempt);
    Duration::from_millis(1u64.saturating_add(random.checked_rem(upper).unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::jitter;
    use std::time::Duration;

    #[test]
    fn a_pause_is_random_within_a_bound_that_grows_with_the_attempt() {
        for attempt in 1..8 {
            let upper = Duration::from_millis(10 << attempt.min(4));
            let pauses: Vec<_> = (0..64).map(|_| jitter(attempt)).collect();
            assert!(
                pauses
                    .iter()
                    .all(|p| *p >= Duration::from_millis(1) && *p <= upper),
                "{pauses:?}"
            );
            assert!(pauses.iter().any(|p| *p > Duration::from_millis(1)));
        }
    }
}
