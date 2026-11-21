//! The small reads and conversions a node start leans on.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Credentials, FsStore, S3Config, S3Store};
use oss_cluster::lease::Membership;
use oss_format::{NodeLease, NodeLogEpoch};

use oss_writer::takeover::Predecessors;

use super::{BootError, NodeConfig, StoreChoice, cluster};
use crate::push_link::Leases;
use crate::writer::{ProactiveConfig, ProactiveTakeover, WriterRole};

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

/// The proactive take of a dead node's shards, off unless the setting is on.
/// A pass runs on a thread of its own, so a slow take never delays a lease;
/// it reads each dead log through the opener's `predecessors`, so a log is
/// fenced once however a shard is taken.
pub(super) fn proactive_takeover(
    config: &NodeConfig,
    role: Arc<WriterRole>,
    predecessors: Arc<Predecessors>,
) -> Arc<ProactiveTakeover> {
    Arc::new(ProactiveTakeover::new(
        &config.node_id,
        ProactiveConfig {
            enabled: config.proactive_takeover,
            reapers: config.proactive_reapers,
            max_concurrent: config.proactive_max_concurrent,
            pass: Duration::from_millis(config.proactive_pass_millis),
            stagger_millis: i64::try_from(config.renew_every().as_millis()).unwrap_or(i64::MAX),
        },
        role,
        predecessors,
    ))
}

/// The leases this node holds, for the forwarder.
#[derive(Clone)]
pub(super) struct Held(pub(super) Arc<Membership>);

impl Leases for Held {
    fn current(&self) -> Arc<[NodeLease]> {
        self.0.current()
    }
}
