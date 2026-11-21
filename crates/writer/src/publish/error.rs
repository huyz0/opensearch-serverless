//! Why a commit was not published.

use std::fmt;

use oss_blob::BlobError;
use oss_format::FormatError;

use crate::pack::PackError;

/// A publish that did not complete.
#[derive(Debug)]
pub enum PublishError {
    /// A newer term holds the register (`StaleWriterException`): this writer
    /// was replaced, and its bytes are inert behind the term container.
    Stale {
        /// This writer's term.
        term: u64,
        /// The term that holds the register, or `None` when the register
        /// was lost without a reading to say.
        found: Option<u64>,
    },
    /// The register holds this term from another writer
    /// (`ForeignWriterException`): the shard-head swap should make that
    /// impossible, and a commit built from two nodes' segments is worse
    /// than a refusal.
    Foreign {
        /// The shared term.
        term: u64,
        /// The writer the register names.
        existing: String,
        /// This writer.
        ours: String,
    },
    /// The object store refused or failed a request.
    Blob(BlobError),
    /// A manifest or header could not be read or written.
    Format(FormatError),
    /// A node pack could not be written.
    Pack(PackError),
    /// The publish's node pack landed longer ago than the swap deadline, so
    /// collection may already have judged it unreferenced; the next publish
    /// packs the files anew.
    PackExpired,
    /// A file of the commit could not be read from its directory.
    Directory(String),
    /// A published manifest this writer cannot build on.
    Unusable(&'static str),
}

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale {
                term,
                found: Some(found),
            } => write!(
                f,
                "term {term} is stale: term {found} holds the commit register"
            ),
            Self::Stale { term, found: None } => {
                write!(f, "term {term} lost the commit register to another writer")
            }
            Self::Foreign {
                term,
                existing,
                ours,
            } => write!(
                f,
                "term {term} is held by writer [{existing}], not [{ours}]"
            ),
            Self::Blob(e) => write!(f, "{e}"),
            Self::Format(e) => write!(f, "{e}"),
            Self::Pack(e) => write!(f, "{e}"),
            Self::PackExpired => {
                write!(f, "the node pack landed too long before the manifest swap")
            }
            Self::Directory(e) => write!(f, "{e}"),
            Self::Unusable(why) => write!(f, "the published commit cannot be built on: {why}"),
        }
    }
}

impl std::error::Error for PublishError {}

impl From<BlobError> for PublishError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<PackError> for PublishError {
    fn from(e: PackError) -> Self {
        Self::Pack(e)
    }
}

impl From<FormatError> for PublishError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}

#[cfg(test)]
mod tests {
    use super::PublishError;

    #[test]
    fn each_refusal_says_what_it_was() {
        let stale = PublishError::Stale {
            term: 4,
            found: Some(5),
        };
        assert_eq!(
            stale.to_string(),
            "term 4 is stale: term 5 holds the commit register"
        );
        let lost = PublishError::Stale {
            term: 4,
            found: None,
        };
        assert_eq!(
            lost.to_string(),
            "term 4 lost the commit register to another writer"
        );
        let foreign = PublishError::Foreign {
            term: 4,
            existing: "a".to_owned(),
            ours: "b".to_owned(),
        };
        assert_eq!(foreign.to_string(), "term 4 is held by writer [a], not [b]");
        assert_eq!(
            PublishError::Directory("x: gone".to_owned()).to_string(),
            "x: gone"
        );
        assert_eq!(
            PublishError::Unusable("why").to_string(),
            "the published commit cannot be built on: why"
        );
    }
}
