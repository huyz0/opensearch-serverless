//! Why a bucket manifest was not read or written.

use std::fmt;

use oss_blob::BlobError;
use oss_format::FormatError;

/// A bucket operation that did not complete.
#[derive(Debug)]
pub enum BucketError {
    /// The object store refused or failed a request.
    Blob(BlobError),
    /// A manifest or commit could not be read or written.
    Format(FormatError),
    /// The bucket has no manifest register.
    NoBucket {
        /// The bucket asked for.
        bucket: String,
    },
    /// The index is not a member of the bucket.
    NotAMember {
        /// The bucket asked.
        bucket: String,
        /// The index uuid asked for.
        uuid: String,
    },
    /// The register moved under every attempt to swap it.
    Busy {
        /// How many swaps were lost.
        attempts: usize,
    },
}

impl fmt::Display for BucketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob(e) => write!(f, "{e}"),
            Self::Format(e) => write!(f, "{e}"),
            Self::NoBucket { bucket } => write!(f, "bucket [{bucket}] has no manifest"),
            Self::NotAMember { bucket, uuid } => {
                write!(f, "index [{uuid}] is not a member of bucket [{bucket}]")
            }
            Self::Busy { attempts } => {
                write!(f, "the bucket manifest kept moving: {attempts} swaps lost")
            }
        }
    }
}

impl std::error::Error for BucketError {}

impl From<BlobError> for BucketError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<FormatError> for BucketError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}
