//! Why a metadata operation did not apply.

use std::fmt;

use oss_blob::BlobError;
use oss_format::{FormatError, SettingsRefused};

use crate::names::NameError;

/// A metadata failure.
#[derive(Debug)]
pub enum MetaError {
    /// The name cannot be an index or alias; nothing was sent.
    InvalidName(NameError),
    /// The name is taken by a live index or alias (the Java's
    /// `IndexAlreadyExistsException`). Uniqueness is the store's to arbitrate:
    /// of racing creates of one name, one applies and the rest end here.
    AlreadyExists,
    /// The name's tombstone was claimed by a sweep under a minute ago, which
    /// may still be removing the old index's data (the Java's
    /// `NameBeingReclaimedException`). Retry after the minute.
    BeingReclaimed,
    /// A record could not be read or written: a malformed register, or a
    /// buffer too short for the record ([`FormatError::TooLarge`] says how
    /// long one must be).
    Format(FormatError),
    /// The store failed.
    Blob(BlobError),
    /// More live names than the cap begin with the prefix (the Java's
    /// `TooManyMatchesException`): the first `cap` would be a partial answer
    /// that looks complete.
    TooManyMatches {
        /// The prefix that matched too much.
        prefix: String,
        /// The cap it passed.
        cap: usize,
    },
    /// A register moved under every attempt to swap it: too many writers
    /// at once.
    Contended,
    /// A bound the caller passed cannot be met, such as a page of no names;
    /// nothing was sent.
    InvalidArgument(&'static str),
    /// A settings update the Java would refuse: one that makes a list,
    /// leaves a list entry `p.0` beside a setting `p`, or grows past
    /// [`oss_format::MAX_CLUSTER_SETTINGS_BYTES`]; nothing was written.
    SettingsRefused(SettingsRefused),
}

impl fmt::Display for MetaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(e) => e.fmt(f),
            Self::AlreadyExists => f.write_str("the name is already taken"),
            Self::BeingReclaimed => f.write_str("the name is being reclaimed from a deleted index"),
            Self::Format(e) => e.fmt(f),
            Self::Blob(e) => e.fmt(f),
            Self::TooManyMatches { prefix, cap } => write!(
                f,
                "more than {cap} indices begin with [{prefix}]. Narrowing the pattern or \
                 naming the indices is the answer; returning the first {cap} would be a \
                 partial result that looks complete."
            ),
            Self::Contended => f.write_str("the register moved under every attempt"),
            Self::InvalidArgument(why) => f.write_str(why),
            Self::SettingsRefused(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for MetaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidName(e) => Some(e),
            Self::Format(e) => Some(e),
            Self::Blob(e) => Some(e),
            Self::SettingsRefused(e) => Some(e),
            Self::AlreadyExists
            | Self::BeingReclaimed
            | Self::TooManyMatches { .. }
            | Self::Contended
            | Self::InvalidArgument(_) => None,
        }
    }
}

impl From<NameError> for MetaError {
    fn from(e: NameError) -> Self {
        Self::InvalidName(e)
    }
}

impl From<FormatError> for MetaError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}

impl From<SettingsRefused> for MetaError {
    fn from(e: SettingsRefused) -> Self {
        Self::SettingsRefused(e)
    }
}

impl From<BlobError> for MetaError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;
    use crate::names::validate_index_or_alias;

    #[test]
    fn every_kind_names_its_cause() {
        let name = validate_index_or_alias("A").unwrap_err();
        let format = FormatError::Corrupt {
            what: "index descriptor",
            reason: "bad",
        };
        let blob = BlobError::InvalidArgument("too long");
        for (error, text, has_source) in [
            (
                MetaError::from(name),
                "Invalid index name [A], must be lowercase",
                true,
            ),
            (MetaError::AlreadyExists, "the name is already taken", false),
            (
                MetaError::BeingReclaimed,
                "the name is being reclaimed from a deleted index",
                false,
            ),
            (
                MetaError::from(format),
                "malformed index descriptor: bad",
                true,
            ),
            (MetaError::from(blob), "invalid argument: too long", true),
            (
                MetaError::TooManyMatches {
                    prefix: "l".to_owned(),
                    cap: 3,
                },
                "more than 3 indices begin with [l]. Narrowing the pattern or naming the \
                 indices is the answer; returning the first 3 would be a partial result that \
                 looks complete.",
                false,
            ),
            (
                MetaError::InvalidArgument("a limit of 0"),
                "a limit of 0",
                false,
            ),
            (
                MetaError::Contended,
                "the register moved under every attempt",
                false,
            ),
            (
                MetaError::from(SettingsRefused::Conflict {
                    prefix: "x".to_owned(),
                    key: "x.0".to_owned(),
                }),
                "settings can't contain values for [x] and [x.0]",
                true,
            ),
        ] {
            assert_eq!(error.to_string(), text);
            assert_eq!(error.source().is_some(), has_source, "{error}");
        }
    }
}
