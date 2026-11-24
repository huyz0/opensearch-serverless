//! The node's lease as its write log writes it: the epochs a successor
//! must fence go into the lease register before an epoch's first blob.
//!
//! The Java's `LeaseWriter` over `BlobLeaseMembership.renew`: the lease as
//! this node last described itself, its `node_log` replaced by the epochs.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use oss_cluster::Membership;
use oss_format::{NodeLease, NodeLogEpoch};
use oss_writer::log::{LeaseWrite, Refusal};

/// What the node holds and may hold, as its lease reports it to peers.
#[derive(Debug)]
pub struct LoadReport {
    held: AtomicI32,
    cap: AtomicI32,
}

impl Default for LoadReport {
    fn default() -> Self {
        // -1 is what a lease says when it does not say.
        Self {
            held: AtomicI32::new(-1),
            cap: AtomicI32::new(-1),
        }
    }
}

impl LoadReport {
    /// Records the shards held now and the cap (0 for none).
    pub fn set(&self, held: usize, cap: usize) {
        self.held
            .store(i32::try_from(held).unwrap_or(i32::MAX), Ordering::Relaxed);
        self.cap
            .store(i32::try_from(cap).unwrap_or(i32::MAX), Ordering::Relaxed);
    }

    /// Puts the load in `lease`.
    pub fn stamp(&self, lease: &mut NodeLease) {
        lease.held = self.held.load(Ordering::Relaxed);
        lease.cap = self.cap.load(Ordering::Relaxed);
    }
}

/// Writes the node's lease through its membership.
pub struct MembershipLeaseWrite {
    membership: Arc<Membership>,
    lease: Mutex<NodeLease>,
    load: Option<Arc<LoadReport>>,
}

impl MembershipLeaseWrite {
    /// A writer renewing `lease` through `membership`.
    pub fn new(membership: Arc<Membership>, lease: NodeLease) -> Self {
        Self {
            membership,
            lease: Mutex::new(lease),
            load: None,
        }
    }

    /// Reports `load` in the lease each write renews.
    #[must_use]
    pub fn with_load(mut self, load: Arc<LoadReport>) -> Self {
        self.load = Some(load);
        self
    }
}

impl LeaseWrite for MembershipLeaseWrite {
    fn write(&self, epochs: &[NodeLogEpoch], _opens_epoch: bool) -> Result<(), Refusal> {
        let mut lease = self.lease.lock().unwrap_or_else(PoisonError::into_inner);
        // ALLOC: one copy of the epochs a lease write, a handful.
        lease.node_log = epochs.to_vec();
        if let Some(load) = &self.load {
            load.stamp(&mut lease);
        }
        self.membership
            .renew(&lease)
            .map(|_| ())
            .map_err(|e| Arc::new(e) as Refusal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // docs/specs/writer-hand-off AC-7
    #[test]
    fn the_load_a_lease_reports_is_what_was_last_set_and_unsaid_before() {
        let load = LoadReport::default();
        let mut lease = NodeLease::new("n", "e", "a", 0);
        load.stamp(&mut lease);
        assert_eq!((lease.held, lease.cap), (-1, -1));
        load.set(7, 100);
        load.stamp(&mut lease);
        assert_eq!((lease.held, lease.cap), (7, 100));
        load.set(3, 0);
        load.stamp(&mut lease);
        assert_eq!((lease.held, lease.cap), (3, 0));
    }
}
