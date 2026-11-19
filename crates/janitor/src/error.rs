//! Why a sweep did not finish.

use std::fmt;

use oss_blob::BlobError;
use oss_cluster::ClusterError;
use oss_format::FormatError;
use oss_meta::MetaError;

/// A sweep failure. Nothing is deleted on one that precedes the delete, and
/// a failed delete leaves the sweep's memory as it was.
#[derive(Debug)]
pub enum JanitorError {
    /// The store failed.
    Blob(BlobError),
    /// The manifest or a key was not what the format allows.
    Format(FormatError),
    /// The pins could not be read: unreadable is not "none".
    Pins(&'static str),
    /// A lease or a shard head could not be read or swapped.
    Cluster(ClusterError),
    /// A descriptor could not be read.
    Meta(MetaError),
    /// The node's write log could not be read, fenced or deleted.
    Log(String),
}

impl fmt::Display for JanitorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob(e) => e.fmt(f),
            Self::Format(e) => e.fmt(f),
            Self::Pins(why) => write!(f, "the pins could not be read: {why}"),
            Self::Cluster(e) => e.fmt(f),
            Self::Meta(e) => e.fmt(f),
            Self::Log(why) => write!(f, "the write log: {why}"),
        }
    }
}

impl std::error::Error for JanitorError {}

impl From<BlobError> for JanitorError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<ClusterError> for JanitorError {
    fn from(e: ClusterError) -> Self {
        Self::Cluster(e)
    }
}

impl From<MetaError> for JanitorError {
    fn from(e: MetaError) -> Self {
        Self::Meta(e)
    }
}

impl From<FormatError> for JanitorError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}

#[cfg(test)]
mod tests {
    use super::JanitorError;

    #[test]
    fn an_unreadable_pin_says_so() {
        let e = JanitorError::Pins("a view record");
        assert_eq!(e.to_string(), "the pins could not be read: a view record");
    }

    #[test]
    fn a_log_failure_and_a_cluster_or_meta_failure_say_what_failed() {
        let e = JanitorError::Log("fence refused".into());
        assert_eq!(e.to_string(), "the write log: fence refused");
        let cluster = oss_cluster::ClusterError::Contended;
        let message = cluster.to_string();
        let e = JanitorError::from(cluster);
        assert!(matches!(e, JanitorError::Cluster(_)));
        assert_eq!(e.to_string(), message);
        let meta = oss_meta::MetaError::AlreadyExists;
        let message = meta.to_string();
        let e = JanitorError::from(meta);
        assert!(matches!(e, JanitorError::Meta(_)));
        assert_eq!(e.to_string(), message);
    }

    #[test]
    fn a_store_or_format_failure_keeps_its_own_message() {
        let blob = oss_blob::BlobError::Stream("cut");
        let message = blob.to_string();
        let e = JanitorError::from(blob);
        assert!(matches!(e, JanitorError::Blob(_)));
        assert_eq!(e.to_string(), message);
        let format = oss_format::FormatError::Truncated {
            what: "register",
            needed: 8,
            have: 1,
        };
        let message = format.to_string();
        let e = JanitorError::from(format);
        assert!(matches!(e, JanitorError::Format(_)));
        assert_eq!(e.to_string(), message);
    }
}
