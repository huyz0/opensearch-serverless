//! The small reads and conversions a node start leans on.

use std::sync::Arc;

use oss_blob::{BlobStore, Credentials, FsStore, S3Config, S3Store};
use oss_format::{NodeLease, NodeLogEpoch};

use super::{BootError, StoreChoice, cluster};

/// The buffer a lease register is read through.
const LEASE_BUFFER: usize = 1 << 16;

/// The store `choice` names.
pub(super) fn store_of(choice: &StoreChoice) -> Result<Arc<dyn BlobStore>, BootError> {
    match choice {
        StoreChoice::Fs { root } => FsStore::new(root)
            .map(|s| Arc::new(s) as Arc<dyn BlobStore>)
            .map_err(|e| BootError::Config(format!("store: {e}"))),
        StoreChoice::S3 {
            endpoint,
            bucket,
            region,
            access_key,
            secret_key,
        } => S3Store::new(S3Config::new(
            endpoint,
            bucket,
            region,
            Credentials::new(access_key, secret_key),
        ))
        .map(|s| Arc::new(s) as Arc<dyn BlobStore>)
        .map_err(|e| BootError::Config(format!("store: {e}"))),
    }
}

/// The write-log epochs the lease register of `node_id` names; none when
/// there is no register.
pub(super) fn earlier_epochs(
    store: &dyn BlobStore,
    base: &str,
    node_id: &str,
) -> Result<Vec<NodeLogEpoch>, BootError> {
    let key = oss_format::lease_key(base, node_id).map_err(cluster)?;
    // ALLOC: one buffer, once a node.
    let mut buf = vec![0u8; LEASE_BUFFER];
    let Some(read) = store.read_register(&key, &mut buf).map_err(cluster)? else {
        return Ok(Vec::new());
    };
    let bytes = buf.get(..read.len).unwrap_or_default();
    NodeLease::parse(bytes)
        .map(|lease| lease.node_log)
        .map_err(cluster)
}

/// The six bytes time-based ids are made with: from the node id, so two
/// nodes do not make one id.
pub(super) fn address_bytes(node_id: &str) -> [u8; 6] {
    let h = oss_cluster::placement::murmur3(&[node_id]).to_be_bytes();
    let g = oss_cluster::placement::murmur3(&[node_id, "#"]).to_be_bytes();
    [h[0], h[1], h[2], h[3], g[0], g[1]]
}
