//! Where a node keeps its blobs.

use std::path::PathBuf;

/// Where a node keeps its blobs.
#[derive(Clone, PartialEq, Eq)]
pub enum StoreChoice {
    /// A directory on this host.
    Fs {
        /// The directory.
        root: PathBuf,
    },
    /// An S3 bucket.
    S3 {
        /// `http://host:port` or `https://host`.
        endpoint: String,
        /// The bucket.
        bucket: String,
        /// The signing region.
        region: String,
        /// The access key id.
        access_key: String,
        /// The secret; never printed.
        secret_key: String,
    },
}
