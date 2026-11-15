//! Why a blob's bytes could not be read as the format they claim.
//!
//! The Java throws an `IOException` with a message built from the bytes; this
//! is one enum whose messages are static, so failing allocates nothing.

use std::fmt;

/// A decode failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The bytes end before the format does. `needed` is how many bytes the
    /// part being read takes from the start, when the format says so (a
    /// header's length prefix), so a caller holding a prefix knows how much
    /// more to fetch; zero when it cannot tell.
    Truncated {
        /// What was being read.
        what: &'static str,
        /// Bytes needed from the start, or zero when unknown.
        needed: u64,
        /// Bytes given.
        have: u64,
    },
    /// The bytes break the format.
    Corrupt {
        /// What was being read.
        what: &'static str,
        /// The rule broken.
        reason: &'static str,
    },
    /// A format version this reader does not know.
    Unsupported {
        /// What was being read.
        what: &'static str,
        /// The version found.
        version: u64,
    },
    /// A count or length past what this reader accepts.
    TooLarge {
        /// What was being read.
        what: &'static str,
        /// The largest accepted.
        limit: u64,
        /// What the bytes claim.
        actual: u64,
    },
    /// A record that is valid, but not one a reader can follow to a commit
    /// in this version: a bucketed index, an alias, or a deleted index.
    NotFollowable {
        /// What was being read.
        what: &'static str,
        /// Why it cannot be followed.
        reason: &'static str,
    },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { what, needed, have } if *needed > 0 => {
                write!(f, "{what} is truncated: needs {needed} bytes, has {have}")
            }
            Self::Truncated { what, have, .. } => write!(f, "{what} is truncated at {have} bytes"),
            Self::Corrupt { what, reason } => write!(f, "malformed {what}: {reason}"),
            Self::Unsupported { what, version } => {
                write!(f, "{what} of unknown format version {version}")
            }
            Self::TooLarge {
                what,
                limit,
                actual,
            } => write!(f, "{what} too large: {actual}, limit {limit}"),
            Self::NotFollowable { what, reason } => {
                write!(f, "{what} cannot be followed: {reason}")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// A corrupt-record error.
pub(crate) const fn corrupt(what: &'static str, reason: &'static str) -> FormatError {
    FormatError::Corrupt { what, reason }
}

/// A length as `u64`, for error fields.
pub(crate) fn len64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}
