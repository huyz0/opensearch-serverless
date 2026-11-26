//! Why a pack was not written.

use std::fmt;

use oss_blob::BlobError;
use oss_format::FormatError;

use super::ledger::MAX_LIVE_EPOCHS;

/// A node pack that did not land.
#[derive(Debug)]
pub enum PackError {
    /// The object store refused or failed a request.
    Blob(BlobError),
    /// A header or ledger could not be written or read.
    Format(FormatError),
    /// The pack's slot is taken: only a reused epoch can cause it
    /// (`FileAlreadyExistsException`).
    Taken,
    /// A part's source gave other bytes than the length it declared.
    Length(&'static str),
    /// The ledger moved under every attempt to update it.
    LedgerBusy,
    /// The ledger already holds the most live epochs a collector tracks, so
    /// the writer will not start another.
    TooManyEpochs,
    /// Another publish's pack write failed, and this one's blobs were in it;
    /// the message is that failure's.
    Shared(String),
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob(e) => write!(f, "{e}"),
            Self::Format(e) => write!(f, "{e}"),
            Self::Taken => write!(f, "the node pack's slot is taken"),
            Self::Length(why) => write!(f, "a pack part {why}"),
            Self::LedgerBusy => write!(f, "the node pack ledger kept moving under every attempt"),
            Self::TooManyEpochs => write!(
                f,
                "the node already has {MAX_LIVE_EPOCHS} live pack epochs, so no more are started"
            ),
            Self::Shared(why) => write!(f, "node pack write failed: {why}"),
        }
    }
}

impl std::error::Error for PackError {}

impl From<BlobError> for PackError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<FormatError> for PackError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}
