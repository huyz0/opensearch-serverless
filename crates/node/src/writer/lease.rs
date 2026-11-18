//! The node's lease as its write log writes it: the epochs a successor
//! must fence go into the lease register before an epoch's first blob.
//!
//! The Java's `LeaseWriter` over `BlobLeaseMembership.renew`: the lease as
//! this node last described itself, its `node_log` replaced by the epochs.

use std::sync::{Arc, Mutex, PoisonError};

use oss_cluster::Membership;
use oss_format::{NodeLease, NodeLogEpoch};
use oss_writer::log::{LeaseWrite, Refusal};

/// Writes the node's lease through its membership.
pub struct MembershipLeaseWrite {
    membership: Arc<Membership>,
    lease: Mutex<NodeLease>,
}

impl MembershipLeaseWrite {
    /// A writer renewing `lease` through `membership`.
    pub fn new(membership: Arc<Membership>, lease: NodeLease) -> Self {
        Self {
            membership,
            lease: Mutex::new(lease),
        }
    }
}

impl LeaseWrite for MembershipLeaseWrite {
    fn write(&self, epochs: &[NodeLogEpoch], _opens_epoch: bool) -> Result<(), Refusal> {
        let mut lease = self.lease.lock().unwrap_or_else(PoisonError::into_inner);
        // ALLOC: one copy of the epochs a lease write, a handful.
        lease.node_log = epochs.to_vec();
        self.membership
            .renew(&lease)
            .map(|_| ())
            .map_err(|e| Arc::new(e) as Refusal)
    }
}
