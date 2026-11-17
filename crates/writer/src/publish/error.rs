//! Why a commit was not published.

use std::fmt;

use oss_blob::BlobError;
use oss_format::FormatError;

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

impl From<FormatError> for PublishError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}
