//! Where a stock restore reads an `s3` source: this node's own endpoint and
//! credentials, with the source's bucket named by the repository.

/// The S3 endpoint and credentials a stock restore reads a source bucket
/// with: this node's own, the bucket being the source's.
#[derive(Clone, Debug)]
pub struct SourceS3 {
    /// `http://host:port` or `https://host`.
    pub endpoint: String,
    /// The signing region.
    pub region: String,
    /// The node's own credentials; `Debug` shows neither key.
    pub credentials: oss_blob::Credentials,
    /// This deployment's own bucket, which a source may not overlap.
    pub own_bucket: String,
}
