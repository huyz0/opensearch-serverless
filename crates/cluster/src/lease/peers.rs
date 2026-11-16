//! Other nodes' leases: reading one, revoking a dead one's, and letting go
//! of a dead one's write-log epochs on its behalf.
//!
//! Ported from `BlobLeaseMembership.java` (`revoke`, `revokeOnce`, `read`,
//! `forgetNodeLog`).

use oss_blob::SwapOutcome;
use oss_format::{MAX_LEASE_BYTES, NodeLease, NodeLogEpoch};

use super::REVOKE_ATTEMPTS;
use super::membership::Membership;
use crate::error::ClusterError;

impl Membership {
    /// Makes sure a node judged dead cannot renew the lease it held, before
    /// one of its shards is taken: its lease swapped, at the generation
    /// read, to a revoked copy. That swap and the owner's renewal contend
    /// on one register, so either the owner renewed first and this answers
    /// false, or the owner's next renewal fails.
    ///
    /// True if the owner can no longer renew: revoked here or already, a
    /// lease released or collected, or one of a later process than
    /// `ephemeral_id`. False if it is live, or its lease could not be read
    /// or swapped: refusing the takeover is the safe answer to both.
    pub fn revoke(&self, node_id: &str, ephemeral_id: Option<&str>) -> bool {
        let Ok(key) = self.lease_key(node_id) else {
            return false;
        };
        // ALLOC: two lease buffers a revocation, MAX_LEASE_BYTES each.
        let (mut read, mut write) = (vec![0u8; MAX_LEASE_BYTES], vec![0u8; MAX_LEASE_BYTES]);
        for _ in 0..REVOKE_ATTEMPTS {
            match self.revoke_once(&key, ephemeral_id, &mut read, &mut write) {
                Ok(Some(decided)) => return decided,
                Ok(None) => {}
                Err(_) => return false,
            }
        }
        false
    }

    /// One read and swap; `None` when the swap lost and the lease must be
    /// read again.
    fn revoke_once(
        &self,
        key: &str,
        ephemeral_id: Option<&str>,
        read: &mut [u8],
        write: &mut [u8],
    ) -> Result<Option<bool>, ClusterError> {
        let Some((generation, lease)) = self.read_lease(key, read)? else {
            return Ok(Some(true));
        };
        if lease.revoked || ephemeral_id.is_some_and(|e| e != lease.ephemeral_id) {
            return Ok(Some(true));
        }
        if !lease.is_expired_at(self.clock.now_millis()) {
            return Ok(Some(false));
        }
        let mut revoked = lease;
        revoked.revoked = true;
        let len = revoked.write(write)?;
        let value = write.get(..len).unwrap_or_default();
        Ok(match self.store.swap_register(key, generation, value)? {
            SwapOutcome::Applied(_) => Some(true),
            SwapOutcome::Conflict(_) => None,
        })
    }

    /// Reads one node's lease directly: for liveness at a takeover, where
    /// freshness matters more than a request.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Blob`] when the store fails; [`ClusterError::Format`]
    /// for a lease that does not parse or an id that cannot be a key.
    pub fn read(&self, node_id: &str) -> Result<Option<NodeLease>, ClusterError> {
        // ALLOC: one lease buffer a read, MAX_LEASE_BYTES.
        let mut buf = vec![0u8; MAX_LEASE_BYTES];
        Ok(self
            .read_lease(&self.lease_key(node_id)?, &mut buf)?
            .map(|(_, lease)| lease))
    }

    /// Stops a node's lease naming write-log epochs once nothing in them is
    /// needed: for a node that is not live, on its behalf, so its lease can
    /// be pruned like any other. Only if the lease names exactly `checked`
    /// and the node is still not live, by one swap; true if the lease names
    /// no epoch afterwards.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Blob`] when the store fails; [`ClusterError::Format`]
    /// for a lease that does not parse.
    pub fn forget_node_log(
        &self,
        node_id: &str,
        checked: &[NodeLogEpoch],
    ) -> Result<bool, ClusterError> {
        let key = self.lease_key(node_id)?;
        // ALLOC: one lease buffer, MAX_LEASE_BYTES.
        let mut buf = vec![0u8; MAX_LEASE_BYTES];
        let Some((generation, mut lease)) = self.read_lease(&key, &mut buf)? else {
            return Ok(true);
        };
        if lease.node_log.is_empty() {
            return Ok(true);
        }
        if lease.node_log != checked
            || (!lease.revoked && !lease.is_expired_at(self.clock.now_millis()))
        {
            return Ok(false);
        }
        // ALLOC: none.
        lease.node_log = Vec::new();
        let len = lease.write(&mut buf)?;
        let value = buf.get(..len).unwrap_or_default();
        Ok(matches!(
            self.store.swap_register(&key, generation, value)?,
            SwapOutcome::Applied(_)
        ))
    }
}
