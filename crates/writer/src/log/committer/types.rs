//! The committer's collaborators, configuration, positions and errors.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use oss_format::{FormatError, NodeLogEpoch, ShardKey};

use crate::log::LogError;

/// A failure a lease write or an admission check reports.
pub type Refusal = Arc<dyn std::error::Error + Send + Sync>;

/// Records the node's open epochs in its lease before an epoch is written.
pub trait EpochLease: Send + Sync {
    /// Writes the lease with `epochs`, every epoch a successor would have
    /// to fence and replay, oldest first; returns once the write landed.
    ///
    /// # Errors
    ///
    /// When the lease could not be written; the epoch is then not used.
    fn record(&self, epochs: &[NodeLogEpoch]) -> Result<(), Refusal>;
}

/// Confirms this node still owns a shard before the shard's first records
/// go into an epoch.
pub trait ShardAdmission: Send + Sync {
    /// Checks the shard's head names this node at `term`; `epoch` is the
    /// one the records are about to go into, already in the lease.
    ///
    /// # Errors
    ///
    /// When this node does not own the shard at that term, or the check
    /// failed.
    fn admit(&self, shard: &ShardKey, term: i64, epoch: u64) -> Result<(), Refusal>;
}

impl<T: EpochLease + ?Sized> EpochLease for Arc<T> {
    fn record(&self, epochs: &[NodeLogEpoch]) -> Result<(), Refusal> {
        T::record(self, epochs)
    }
}

impl<T: ShardAdmission + ?Sized> ShardAdmission for Arc<T> {
    fn admit(&self, shard: &ShardKey, term: i64, epoch: u64) -> Result<(), Refusal> {
        T::admit(self, shard, term, epoch)
    }
}

/// Where a shard's records landed: in this blob, and its earlier records in
/// it or before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    /// The epoch.
    pub epoch: u64,
    /// The blob's slot.
    pub ordinal: u64,
}

/// Why a shard's records did not land.
#[derive(Clone, Debug)]
pub enum CommitError {
    /// The slot was fenced: a successor closed the epoch.
    Fenced {
        /// The epoch.
        epoch: u64,
        /// The slot the fence took.
        ordinal: u64,
    },
    /// The append failed or its outcome is unknown.
    Store(LogError),
    /// The epoch could not be recorded in the lease; nothing was logged.
    Lease(Refusal),
    /// This node may not log to the shard at that term.
    Refused(Refusal),
    /// A record or the blob could not be encoded; nothing was written.
    Encode(FormatError),
    /// The window's writer stopped without an outcome; nothing may read it
    /// as landed.
    Lost,
}

impl fmt::Display for CommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fenced { epoch, ordinal } => {
                write!(
                    f,
                    "epoch {epoch} of the node log was fenced at slot {ordinal}"
                )
            }
            Self::Store(e) => write!(f, "node-log append failed: {e}"),
            Self::Lease(e) => write!(
                f,
                "could not record a new epoch of the node log in the lease; nothing was logged: {e}"
            ),
            Self::Refused(e) => write!(f, "refused to log to the shard: {e}"),
            Self::Encode(e) => write!(f, "could not encode a node-log blob: {e}"),
            Self::Lost => f.write_str("the node-log write stopped without an outcome"),
        }
    }
}

impl std::error::Error for CommitError {}

/// How a committer forms windows; the defaults are the Java's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitterConfig {
    /// How long a window stays open after its first record; zero writes as
    /// soon as nothing is in flight.
    pub window: Duration,
    /// Roughly the most bytes of id and source in one window; at least 1.
    pub max_bytes: u64,
    /// The most records in one window; at least 1.
    pub max_records: u64,
}

impl Default for CommitterConfig {
    fn default() -> Self {
        Self {
            window: Duration::from_millis(250),
            max_bytes: 8 * 1024 * 1024,
            max_records: 50_000,
        }
    }
}

pub(crate) fn epoch_entry(epoch: u64, trim: u64, landed: u64) -> NodeLogEpoch {
    let i = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
    NodeLogEpoch {
        epoch: i(epoch),
        trim: i(trim),
        landed: i(landed),
    }
}
