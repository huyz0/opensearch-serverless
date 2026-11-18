//! Why a mapping did not grow.

use std::fmt;

use crate::mapping::MappingError;

/// A growth that did not complete.
#[derive(Clone, Debug, PartialEq)]
pub enum GrowthError {
    /// The mapping refuses the addition (`MappingConflictException`): the
    /// write fails and retrying it as it is cannot help.
    Conflict(MappingError),
    /// The index is gone, is not an index, or was recreated under another
    /// uuid.
    Deleted(String),
    /// Every attempt lost its swap to another writer: retry the write.
    Concurrent(String),
    /// The descriptor could not be read or written.
    Store(String),
}

impl fmt::Display for GrowthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(e) => write!(f, "{}: {}", e.ty, e.reason),
            Self::Deleted(m) | Self::Concurrent(m) | Self::Store(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for GrowthError {}
