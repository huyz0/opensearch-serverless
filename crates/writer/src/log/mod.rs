//! One node's write log in the object store: the blobs of each epoch, read
//! and written by name.
//!
//! A blob lives at `nodelog/<node>/e=<epoch:020>/<ordinal:020>`
//! ([`oss_format::node_log_key`]); ordinals within an epoch start at zero
//! and have no gaps. A writer knows the next name, a successor learns the
//! open epochs from the node's lease, and cleanup deletes by name, so
//! nothing here lists. Appends are create-if-absent, so a fence in a slot
//! stops the writer there.

mod charge;
mod committer;
mod epochs;
mod fence;
mod hedge;
mod hedge_delay;
mod keeper;
pub mod settings;
mod store;
mod takeover;
mod window;

use std::fmt;

use oss_blob::BlobError;
use oss_format::FormatError;

pub use committer::{
    AckTimings, CommitError, CommitterConfig, EpochLease, NodeLogCommitter, Position, Refusal,
    ShardAdmission,
};
pub use hedge::{HedgeOutcome, Spawn, ThreadPerPut};
pub use keeper::{LeaseKeeper, LeaseWrite, RenewError};
pub use store::{HEADER_PROBE_BYTES, MAX_READ_BYTES, NodeLogStore};
pub use takeover::{BodyCache, LeaseReader, MAX_REFENCES, NodeLogTakeover, TakeoverError};

/// A lease's epoch, trim point or landed hint as a number of the log;
/// a lease never holds a negative one.
pub(crate) fn lease_slot(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// The most slots a fence tries before giving up on a writer that keeps
/// taking the next one (the Java's `FENCE_ATTEMPTS`).
pub const FENCE_ATTEMPTS: u32 = 1000;

/// The widest gap a fence leaves ahead of a writer it keeps losing to.
pub const MAX_FENCE_GAP: u64 = 1024;

/// Why a log operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogError {
    /// The slot is taken by something other than this blob: a fence, so the
    /// epoch is closed to this writer (the Java's
    /// `FileAlreadyExistsException`).
    Taken,
    /// The store failed. After a failed append the outcome is unknown: the
    /// writer must not use the epoch again.
    Blob(BlobError),
    /// A blob or key broke the format.
    Format(FormatError),
    /// A read came back with fewer bytes than the header says it holds.
    Short {
        /// Bytes the header says.
        expected: u64,
        /// Bytes read.
        actual: u64,
    },
    /// A fence found the writer taking the next slot [`FENCE_ATTEMPTS`]
    /// times.
    FenceExhausted,
    /// A slot or range past what the log can name.
    OutOfRange(&'static str),
    /// A header longer than the reader accepts, found from its prefix
    /// before the rest was read.
    HeaderTooLarge {
        /// The header's length.
        bytes: u64,
        /// What the reader accepts.
        cap: u64,
    },
}

impl fmt::Display for LogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Taken => f.write_str("the node-log slot is taken: the epoch is fenced"),
            Self::Blob(e) => write!(f, "node-log store request failed: {e}"),
            Self::Format(e) => write!(f, "node-log blob: {e}"),
            Self::Short { expected, actual } => {
                write!(f, "node-log read of {expected} bytes came back as {actual}")
            }
            Self::FenceExhausted => {
                f.write_str("could not fence the epoch: its writer kept taking the next slot")
            }
            Self::OutOfRange(what) => write!(f, "node-log {what} out of range"),
            Self::HeaderTooLarge { bytes, cap } => {
                write!(f, "node-log header of {bytes} bytes is over the {cap} cap")
            }
        }
    }
}

impl std::error::Error for LogError {}

impl From<BlobError> for LogError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<FormatError> for LogError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}
