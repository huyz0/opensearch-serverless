//! The node's lease as its write log needs it: one lock across computing
//! the epochs and writing them, and the deletes that may follow a lease
//! once it has landed.
//!
//! A successor's fence searches for the first free slot from the landed
//! hint a lease records, trusting every slot below it is taken. So a slot
//! is deleted only once a lease naming a trim point above it has landed,
//! and an epoch no longer written and holding nothing unpublished is left
//! out of a lease first, then deleted whole. All of it is by name, with no
//! listing, and a delete is not billed.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use oss_format::NodeLogEpoch;

use crate::log::{EpochLease, LogError, NodeLogCommitter, NodeLogStore, Refusal};

/// Slots past a complete epoch's landed hint that are deleted with it: the
/// one an append of unknown outcome may have taken, and a fence after it.
const PAST_LANDED: u64 = 2;

/// Writes this node's lease naming the given epochs.
pub trait LeaseWrite: Send + Sync {
    /// Writes the lease with `epochs`, oldest first; returns once it
    /// landed. `opens_epoch` is true when the write records a new epoch
    /// before its first blob, false for a renewal.
    ///
    /// # Errors
    ///
    /// When the lease could not be written.
    fn write(&self, epochs: &[NodeLogEpoch], opens_epoch: bool) -> Result<(), Refusal>;
}

impl<T: LeaseWrite + ?Sized> LeaseWrite for Arc<T> {
    fn write(&self, epochs: &[NodeLogEpoch], opens_epoch: bool) -> Result<(), Refusal> {
        T::write(self, epochs, opens_epoch)
    }
}

/// Why a renewal did not finish.
#[derive(Debug, Clone)]
pub enum RenewError {
    /// The lease was not written; nothing was deleted.
    Lease(Refusal),
    /// The lease landed, then a delete behind it failed; it is tried again
    /// at the next renewal. What is left costs storage, never correctness.
    Delete(LogError),
}

impl fmt::Display for RenewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lease(e) => write!(f, "the lease was not written: {e}"),
            Self::Delete(e) => write!(f, "a delete behind the lease failed: {e}"),
        }
    }
}

impl std::error::Error for RenewError {}

/// What is left to delete behind the leases written so far.
#[derive(Default)]
struct Trim {
    /// Epochs no longer named by the lease and not yet deleted, oldest
    /// first.
    completed: VecDeque<NodeLogEpoch>,
    /// Per epoch still named, the slot below which all is deleted.
    deleted_below: BTreeMap<u64, u64>,
}

/// The committer's [`EpochLease`], and what renews the lease and deletes
/// behind it.
pub struct LeaseKeeper {
    writer: Box<dyn LeaseWrite>,
    store: NodeLogStore,
    inherited: Vec<NodeLogEpoch>,
    trim: Mutex<Trim>,
}

impl LeaseKeeper {
    /// A keeper writing through `writer`, deleting from `store`, and naming
    /// `inherited` (the epochs an earlier process of this node left, which
    /// it never deletes) ahead of this process's own in every lease.
    #[must_use]
    pub fn new(
        writer: Box<dyn LeaseWrite>,
        store: NodeLogStore,
        inherited: Vec<NodeLogEpoch>,
    ) -> Self {
        Self {
            writer,
            store,
            inherited,
            trim: Mutex::new(Trim::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Trim> {
        self.trim.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn with_inherited(&self, epochs: &[NodeLogEpoch]) -> Vec<NodeLogEpoch> {
        // ALLOC: the inherited and open epochs, a handful.
        let mut out = Vec::with_capacity(self.inherited.len().saturating_add(epochs.len()));
        out.extend_from_slice(&self.inherited);
        out.extend_from_slice(epochs);
        out
    }

    /// Writes the lease with the epochs as they stand, then deletes what it
    /// says nobody needs: in each epoch it names, the slots below its trim
    /// point; and each complete epoch whole.
    ///
    /// # Errors
    ///
    /// [`RenewError::Lease`] when the lease was not written, and then
    /// nothing was deleted; [`RenewError::Delete`] when it landed and a
    /// delete failed.
    pub fn renew(&self, committer: &NodeLogCommitter) -> Result<(), RenewError> {
        let mut trim = self.lock();
        trim.completed.extend(committer.take_complete());
        let mine = committer.positions();
        self.writer
            .write(&self.with_inherited(&mine), false)
            .map_err(RenewError::Lease)?;
        self.delete_behind(&mut trim, &mine)
            .map_err(RenewError::Delete)
    }

    fn delete_behind(&self, trim: &mut Trim, named: &[NodeLogEpoch]) -> Result<(), LogError> {
        for entry in named {
            let epoch = slot(entry.epoch);
            let from = trim.deleted_below.get(&epoch).copied().unwrap_or(0);
            let below = slot(entry.trim);
            if below > from {
                self.store.delete_range(epoch, from, below)?;
                trim.deleted_below.insert(epoch, below);
            }
        }
        while let Some(entry) = trim.completed.front() {
            let epoch = slot(entry.epoch);
            let from = trim.deleted_below.get(&epoch).copied().unwrap_or(0);
            let to = slot(entry.landed).saturating_add(PAST_LANDED);
            self.store.delete_range(epoch, from, to)?;
            trim.deleted_below.remove(&epoch);
            trim.completed.pop_front();
        }
        Ok(())
    }
}

impl EpochLease for LeaseKeeper {
    fn record(&self, epochs: &[NodeLogEpoch]) -> Result<(), Refusal> {
        let _trim = self.lock();
        self.writer.write(&self.with_inherited(epochs), true)
    }
}

/// A lease number as a slot or epoch; they are never negative.
fn slot(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}
