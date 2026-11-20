//! A node started from configuration: its store, memory, lease, write log
//! and roles, assembled and listening.
//!
//! Spec: docs/specs/node-boot/SPEC.md. The lease is liveness only: the shard
//! head's conditional write is what fences a writer, so a late renewal can
//! cost a handover and never a write.

mod config;
mod renew;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Credentials, FsStore, S3Config, S3Store};
use oss_cluster::heads::ShardHeadStore;
use oss_cluster::{LeaseClock, LeaseConfig, LivenessOracle, Membership, SystemClock};
use oss_format::NodeLease;
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore};
use oss_rest::ServerConfig;
use oss_writer::log::{CommitterConfig, LeaseKeeper, NodeLogCommitter, NodeLogStore};

use crate::assemble::{self, AssembleError, Config, Running, Writer};
use crate::descriptor_routes::DescriptorRoutes;
use crate::health_route::StoreHealth;
use crate::writer::{
    DescriptorIncarnation, HeadAdmission, HeadOpener, MembershipLeaseWrite, ShardNames,
};

pub use config::{MIN_MEMORY_BYTES, NodeConfig, StoreChoice};
use renew::Renewal;

/// The roles a node's lease names.
const ROLES: [&str; 2] = ["ingest", "search"];

/// The most members a node's membership view holds.
const MAX_MEMBERS: usize = 1024;

/// The buffer a descriptor, a head or a lease is read through.
const READ_BUFFER: usize = 1 << 20;

/// How long a held shard's index incarnation is trusted before it is read again.
const INCARNATION_WINDOW: Duration = Duration::from_secs(600);

/// Why a node did not start.
#[derive(Debug)]
pub enum BootError {
    /// The configuration does not fit: the budget or the store.
    Config(String),
    /// The store, the lease or a head could not be reached.
    Cluster(String),
    /// The roles could not be assembled.
    Assemble(AssembleError),
    /// The listener could not be bound.
    Bind(String),
}

impl std::fmt::Display for BootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "configuration: {e}"),
            Self::Cluster(e) => write!(f, "cluster: {e}"),
            Self::Assemble(e) => write!(f, "assembly: {e}"),
            Self::Bind(e) => write!(f, "bind: {e}"),
        }
    }
}

impl std::error::Error for BootError {}

/// A node running: dropping it stops its lease renewal and releases its lease.
pub struct Node {
    running: Option<Running>,
    renewal: Option<Renewal>,
    membership: Arc<Membership>,
    node_id: String,
}

impl Node {
    /// The address the node's HTTP server listens on.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.running.as_ref().map_or_else(
            || std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
            Running::addr,
        )
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        // The renewal stops first, so it cannot write the lease back.
        drop(self.renewal.take());
        drop(self.running.take());
        // Best effort: an unreleased lease lapses in one lifetime.
        let _ = self.membership.release(&self.node_id);
    }
}

fn store_of(choice: &StoreChoice) -> Result<Arc<dyn BlobStore>, BootError> {
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

fn cluster<E: std::fmt::Display>(e: E) -> BootError {
    BootError::Cluster(e.to_string())
}

/// Starts a node from `config`: enrols its lease, builds its write log and
/// roles, and listens on the configured HTTP address.
///
/// # Errors
///
/// A [`BootError`] naming what could not be set up.
pub fn start(config: &NodeConfig) -> Result<Node, BootError> {
    let store = store_of(&config.store)?;
    let budget = MemoryBudget::new(
        &config
            .budget()
            .map_err(|e| BootError::Config(e.to_string()))?,
    );
    let clock: Arc<dyn LeaseClock> = Arc::new(SystemClock);
    let membership = Arc::new(
        Membership::new(
            Arc::clone(&store),
            &config.base,
            Arc::clone(&clock),
            LeaseConfig {
                ttl_millis: config.lease_ttl_millis,
                max_members: MAX_MEMBERS,
                conditional_deletes: false,
            },
        )
        .map_err(cluster)?,
    );
    let ephemeral = format!("{:x}-{:x}", clock.now_millis(), std::process::id());

    // The epochs an earlier process of this node left: this one writes above
    // them and names them in every lease, so a successor still fences them.
    let _ = membership.refresh();
    let inherited = membership
        .current()
        .iter()
        .find(|l| l.node_id == config.node_id)
        .map(|l| l.node_log.clone())
        .unwrap_or_default();
    let first_epoch = inherited
        .iter()
        .map(|e| u64::try_from(e.epoch).unwrap_or(0))
        .max()
        .map_or(1, |m| m.saturating_add(1));
    let mut lease = NodeLease::new(&config.node_id, &ephemeral, &config.advertise, 0);
    lease.set_roles(ROLES.map(str::to_owned));
    lease.node_log = inherited.clone();
    membership.renew(&lease).map_err(cluster)?;

    let oracle = Arc::new(LivenessOracle::new(Arc::clone(&membership)));
    let heads = Arc::new(
        ShardHeadStore::new(
            Arc::clone(&store),
            &config.base,
            Arc::clone(&clock),
            config.lease_ttl_millis,
            Arc::clone(&oracle),
        )
        .map_err(cluster)?,
    );
    let descriptors = Arc::new(
        DescriptorStore::new(Arc::clone(&store), &config.base, DeleteMode::Tombstone)
            .map_err(cluster)?,
    );
    let names = Arc::new(ShardNames::default());
    let log =
        NodeLogStore::new(Arc::clone(&store), &config.base, &config.node_id).map_err(cluster)?;
    let keeper = Arc::new(LeaseKeeper::new(
        Box::new(MembershipLeaseWrite::new(Arc::clone(&membership), lease)),
        log.clone(),
        inherited,
    ));
    let committer = Arc::new(NodeLogCommitter::new(
        log,
        first_epoch,
        Box::new(Arc::clone(&keeper)),
        Box::new(HeadAdmission::new(
            Arc::clone(&heads),
            Arc::clone(&names),
            &config.node_id,
            &ephemeral,
        )),
        CommitterConfig::default(),
    ));
    let opener = HeadOpener::new(
        Arc::clone(&heads),
        Arc::clone(&descriptors),
        Arc::clone(&store),
        &config.base,
        &config.node_id,
        &ephemeral,
        READ_BUFFER,
    );
    let routes = DescriptorRoutes::new(
        DescriptorStore::new(Arc::clone(&store), &config.base, DeleteMode::Tombstone)
            .map_err(cluster)?,
        READ_BUFFER,
    );
    let writer = Writer {
        committer: Arc::clone(&committer),
        routes: Arc::new(routes),
        opener: Some(Arc::new(opener)),
        names: Some(Arc::clone(&names)),
        incarnation: Some((
            Arc::new(DescriptorIncarnation::new(Arc::clone(&descriptors))),
            INCARNATION_WINDOW,
        )),
    };
    let mut node_config = Config::new(
        &config.base,
        &config.node_id,
        address_bytes(&config.node_id),
    );
    node_config.publish_every = Duration::from_millis(250);
    let health = Arc::new(StoreHealth::new(
        Arc::clone(&membership),
        oracle,
        ShardHeadStore::new(
            Arc::clone(&store),
            &config.base,
            Arc::clone(&clock),
            config.lease_ttl_millis,
            Arc::new(LivenessOracle::new(Arc::clone(&membership))),
        )
        .map_err(cluster)?,
        DescriptorStore::new(Arc::clone(&store), &config.base, DeleteMode::Tombstone)
            .map_err(cluster)?,
        Arc::clone(&clock),
    ));
    let assembled = assemble::assemble(&node_config, store, &budget, writer)
        .and_then(|a| a.with_health(health, &config.cluster))
        .map_err(BootError::Assemble)?;
    let listener = TcpListener::bind(config.http)
        .map_err(|e| BootError::Bind(format!("{}: {e}", config.http)))?;
    let running = assembled
        .start(listener, ServerConfig::default(), &budget)
        .map_err(BootError::Assemble)?;

    let renewing = Arc::clone(&membership);
    let renewal = Renewal::spawn(config.renew_every(), move || {
        // A failed tick is retried by the next: the lease only has to land
        // once in a lifetime.
        let _ = keeper.renew(&committer);
        let _ = renewing.refresh();
    });
    Ok(Node {
        running: Some(running),
        renewal: Some(renewal),
        membership,
        node_id: config.node_id.clone(),
    })
}

/// The six bytes time-based ids are made with: from the node id, so two
/// nodes do not make one id.
fn address_bytes(node_id: &str) -> [u8; 6] {
    let h = oss_cluster::placement::murmur3(&[node_id]).to_be_bytes();
    let g = oss_cluster::placement::murmur3(&[node_id, "#"]).to_be_bytes();
    [h[0], h[1], h[2], h[3], g[0], g[1]]
}
