//! Why a membership operation did not apply.

use std::fmt;

use oss_blob::BlobError;
use oss_format::FormatError;

/// A membership failure.
#[derive(Debug)]
pub enum ClusterError {
    /// The store failed.
    Blob(BlobError),
    /// A register held bytes that are not the record it should be, or a
    /// record did not fit its bound.
    Format(FormatError),
    /// A successor revoked this node's lease (the Java's
    /// `LeaseRevokedException`): it may have taken shards this node holds,
    /// so the caller treats it as a lapse. The next renewal starts afresh.
    Revoked {
        /// The node whose lease was revoked.
        node_id: String,
    },
    /// Another live process holds this node id's lease at another address:
    /// a cloned data directory or a bypassed node lock. This node refuses
    /// to renew rather than share every shard with it.
    Duplicate {
        /// The node id both claim.
        node_id: String,
        /// The other process's ephemeral id.
        other_ephemeral_id: String,
        /// The other process's address.
        other_address: String,
    },
    /// The register moved again under the retry after a conflict.
    Contended,
    /// The transport secret could not be minted or read back.
    TransportSecret(&'static str),
    /// A shard head vanished between a lost race and the read of its winner.
    Heads(&'static str),
}

impl fmt::Display for ClusterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob(e) => e.fmt(f),
            Self::Format(e) => e.fmt(f),
            Self::Revoked { node_id } => {
                write!(f, "the lease of [{node_id}] was revoked by a successor")
            }
            Self::Duplicate {
                node_id,
                other_ephemeral_id,
                other_address,
            } => write!(
                f,
                "duplicate node id [{node_id}]: another live process ({other_ephemeral_id} at \
                 {other_address}) holds this node's lease; this node will not renew"
            ),
            Self::Contended => f.write_str("the lease register moved under every attempt"),
            Self::TransportSecret(why) | Self::Heads(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ClusterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Blob(e) => Some(e),
            Self::Format(e) => Some(e),
            _ => None,
        }
    }
}

impl From<BlobError> for ClusterError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<FormatError> for ClusterError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}
