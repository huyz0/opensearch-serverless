//! Why bytes could not be read or written as the transport wire.
//!
//! The Java throws an `IOException` or `IllegalStateException` with a message
//! built from the bytes; this is one enum with static messages, so failing
//! allocates nothing.

use std::fmt;

/// A wire encode or decode failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The bytes end before the value does.
    Truncated {
        /// What was being read.
        what: &'static str,
    },
    /// The bytes break the encoding.
    Malformed {
        /// What was being read.
        what: &'static str,
        /// The rule broken.
        reason: &'static str,
    },
    /// A length, count or output past what is accepted.
    TooLarge {
        /// What was being read or written.
        what: &'static str,
        /// The largest accepted.
        limit: u64,
        /// What was asked for, or the least that was, when the whole is
        /// not known.
        actual: u64,
    },
    /// A peer at a version this node does not speak.
    UnsupportedVersion {
        /// The peer's wire version id.
        id: u32,
    },
}

impl WireError {
    pub(crate) fn malformed(what: &'static str, reason: &'static str) -> Self {
        Self::Malformed { what, reason }
    }

    pub(crate) fn too_large(what: &'static str, limit: usize, actual: usize) -> Self {
        Self::TooLarge {
            what,
            limit: u64::try_from(limit).unwrap_or(u64::MAX),
            actual: u64::try_from(actual).unwrap_or(u64::MAX),
        }
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { what } => write!(f, "{what} is truncated"),
            Self::Malformed { what, reason } => write!(f, "{what} is malformed: {reason}"),
            Self::TooLarge {
                what,
                limit,
                actual,
            } => {
                write!(f, "{what} is too large: {actual} past the limit of {limit}")
            }
            Self::UnsupportedVersion { id } => {
                write!(f, "a peer at wire version {id} is not supported")
            }
        }
    }
}

impl std::error::Error for WireError {}
