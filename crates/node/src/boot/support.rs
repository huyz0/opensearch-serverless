//! The small reads and conversions a node start leans on.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Credentials, FsStore, S3Config, S3Store};
use oss_cluster::lease::Membership;
use oss_format::{NodeLease, NodeLogEpoch};

use oss_writer::takeover::Predecessors;

use super::{BootError, NodeConfig, StoreChoice, cluster};
use crate::cluster_owner::IndexSource;
use crate::descriptor_routes::DescriptorRoutes;
use crate::push_link::Leases;
use crate::writer::{
    LiveLeases, OrphanParts, OrphanTimer, ProactiveConfig, ProactiveTakeover, Routes,
    TombstoneParts, TombstonePass, WriterRole,
};
use oss_cluster::LeaseClock;
use oss_mem::MemoryBudget;
use oss_meta::DeleteMode;

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

/// The pass that removes the tombstones of deleted indexes, which the one
/// node ranked to sweep runs once every `OSS_TOMBSTONE_SWEEP_MS`.
pub(super) fn tombstone_pass(
    config: &NodeConfig,
    store: &Arc<dyn BlobStore>,
    budget: &MemoryBudget,
    clock: Arc<dyn LeaseClock>,
) -> TombstonePass {
    let proving = Arc::clone(store);
    let base = config.base.clone();
    TombstonePass::new(
        TombstoneParts {
            store: Arc::clone(store),
            base: config.base.clone(),
            node_id: config.node_id.clone(),
            budget: budget.clone(),
            buffer_bytes: LEASE_BUFFER,
            every_millis: i64::try_from(config.tombstone_sweep_millis).unwrap_or(i64::MAX),
            probe: Box::new(move || DeleteMode::probe(proving.as_ref(), &base)),
        },
        Box::new(move || clock.now_millis()),
    )
}

/// The leases this node holds, for the forwarder.
#[derive(Clone)]
pub(super) struct Held(pub(super) Arc<Membership>);

impl Leases for Held {
    fn current(&self) -> Arc<[NodeLease]> {
        self.0.current()
    }
}

impl IndexSource for DescriptorRoutes {
    fn index(&self, name: &str) -> Option<oss_writer::bulk::IndexInfo> {
        Routes::index(self, name)
    }
}

/// The orphan-shard sweep, which the one node ranked to sweep runs once every
/// `OSS_ORPHAN_SLICE_MS`; nothing for a budget of 0.
pub(super) fn orphan_sweep(
    config: &NodeConfig,
    store: &Arc<dyn BlobStore>,
    budget: &MemoryBudget,
    clock: Arc<dyn LeaseClock>,
    live: Arc<LiveLeases>,
    dry_run: bool,
) -> Option<OrphanTimer> {
    OrphanTimer::spawn(
        OrphanParts {
            store: Arc::clone(store),
            base: config.base.clone(),
            node_id: config.node_id.clone(),
            budget: budget.clone(),
            slice: config.orphan_slice_budget,
            every: Duration::from_millis(config.orphan_slice_millis),
            dry_run,
            clock: Arc::new(move || u64::try_from(clock.now_millis()).unwrap_or(0)),
        },
        live,
    )
}

/// The janitor's rhythm and switch and the hibernation threshold, from the
/// node's configuration.
pub(super) fn apply_janitor_and_hibernation(
    node: &mut crate::assemble::Config,
    config: &NodeConfig,
) {
    node.hibernate_after_millis = config.hibernate_after_millis;
    node.give_back_after_millis = config.give_back_after_millis;
    node.max_shards = config.max_shards;
    node.evict_after_millis = config.evict_after_millis;
    node.hand_off = config.hand_off;
    node.knn = config.knn;
    node.evict_headroom_percent = config.evict_headroom_percent;
    node.pinned_indexes.clone_from(&config.pinned_indexes);
    node.janitor_every = Duration::from_millis(config.janitor_every_millis);
    node.sweep.dry_run = config.janitor_dry_run;
}

/// Starts the commit push: this node's writer tells its own reader of each
/// commit it publishes, and the readers of the shard through the transport.
pub(super) fn wire_commit_push(
    assembled: &crate::assemble::Assembled,
    client: &Arc<oss_transport::client::Client>,
    auth: &Arc<oss_transport::auth::Authenticator>,
    membership: &Arc<Membership>,
    config: &NodeConfig,
) -> Result<(), BootError> {
    use crate::push_link::{PlacementTargets, TransportPushLink};
    use crate::push_sender::PushSender;
    /// Readers a commit is pushed to besides the owner.
    const READERS: usize = 2;
    let targets = PlacementTargets::new(Held(Arc::clone(membership)), &config.node_id, READERS);
    let link = TransportPushLink::new(
        Arc::clone(client),
        Arc::clone(auth),
        Held(Arc::clone(membership)),
    );
    let sink = crate::commit_push::CommitPush::start(
        PushSender::new(targets, link, true),
        Arc::clone(&assembled.search) as Arc<dyn crate::commit_push::LocalTake>,
        config.commit_push,
    )
    .map_err(|e| BootError::Cluster(format!("commit push thread: {e}")))?;
    assembled.role.set_commit_sink(sink);
    Ok(())
}

/// What the renewal thread does for the fleet's sake: reports the node's
/// load in the lease it renews, and hands idle shards off after a refresh.
pub(super) struct FleetTick {
    role: Arc<crate::writer::WriterRole>,
    load: Arc<crate::writer::LoadReport>,
    clock: Arc<dyn LeaseClock>,
    ttl_millis: i64,
    node_id: String,
}

impl FleetTick {
    pub(super) fn new(
        role: &Arc<crate::writer::WriterRole>,
        load: Arc<crate::writer::LoadReport>,
        clock: &Arc<dyn LeaseClock>,
        config: &NodeConfig,
    ) -> Self {
        Self {
            role: Arc::clone(role),
            load,
            clock: Arc::clone(clock),
            ttl_millis: config.lease_ttl_millis,
            // ALLOC: one id, once a node.
            node_id: config.node_id.clone(),
        }
    }

    /// Before a renewal: the load the lease is about to say.
    pub(super) fn renewing(&self) {
        let stats = self.role.shard_cap_stats();
        self.load.set(stats.held, stats.max);
    }

    /// After a refresh: hands idle shards to a member with room, when set.
    pub(super) fn refreshed(&self, membership: &Membership) {
        self.role.hand_off(
            &membership.current(),
            self.clock.now_millis(),
            self.ttl_millis,
            &self.node_id,
        );
    }
}

/// The lease this node first describes itself with.
pub(super) fn own_lease(
    config: &NodeConfig,
    ephemeral: &str,
    advertise: &str,
    inherited: &[NodeLogEpoch],
) -> NodeLease {
    let mut lease = NodeLease::new(&config.node_id, ephemeral, advertise, 0);
    lease.version = Some(format!(
        "{}{}",
        crate::read_forward::BUILD_PREFIX,
        env!("CARGO_PKG_VERSION")
    ));
    lease.set_roles(super::ROLES.map(str::to_owned));
    lease.node_log = inherited.to_vec();
    lease
}

/// The bucket wiring, only when `serverless.buckets.enabled` is on.
pub(super) fn bucket_wiring(
    config: &NodeConfig,
    store: &Arc<dyn BlobStore>,
    descriptors: &Arc<oss_meta::DescriptorStore>,
    clock: &Arc<dyn LeaseClock>,
) -> Result<Option<Arc<crate::writer::BucketWiring>>, BootError> {
    use oss_writer::bucket::settings;
    if !config.buckets_enabled {
        return Ok(None);
    }
    let defaults = &oss_types::settings::Empty;
    let bad = |e: oss_types::settings::SettingError| BootError::Config(format!("buckets: {e}"));
    let write = settings::manifest_write(defaults).map_err(bad)?;
    let limits = settings::limits(defaults).map_err(bad)?;
    Ok(Some(Arc::new(crate::writer::BucketWiring::new(
        Arc::clone(store),
        &config.base,
        Arc::clone(descriptors),
        Arc::clone(clock),
        write,
        limits,
    ))))
}

/// The shard head store, routing bucketed members when the wiring is built.
pub(super) fn head_store(
    config: &NodeConfig,
    store: &Arc<dyn BlobStore>,
    clock: &Arc<dyn LeaseClock>,
    oracle: &Arc<oss_cluster::LivenessOracle>,
    buckets: Option<&Arc<crate::writer::BucketWiring>>,
) -> Result<Arc<oss_cluster::heads::ShardHeadStore>, BootError> {
    let mut heads = oss_cluster::heads::ShardHeadStore::new(
        Arc::clone(store),
        &config.base,
        Arc::clone(clock),
        config.lease_ttl_millis,
        Arc::clone(oracle),
    )
    .map_err(cluster)?;
    if let Some(buckets) = buckets {
        heads = heads.with_bucket_heads(buckets.heads());
    }
    Ok(Arc::new(heads))
}

/// The ledger and classifier of `config`'s store usage, when it is on.
pub(super) fn usage_of(
    store: Arc<dyn BlobStore>,
    config: &NodeConfig,
) -> (Arc<dyn BlobStore>, Option<Arc<oss_blob::usage::Ledger>>) {
    if !config.store_usage {
        return (store, None);
    }
    let ledger = Arc::new(oss_blob::usage::Ledger::new(config.store_usage_max_indices));
    let classifier = Arc::new(KeyOwner(config.base.clone())); // ALLOC: one name, at start.
    let wrapped = oss_blob::UsageStore::new(
        oss_blob::Shared::new(store),
        Arc::clone(&ledger),
        classifier,
    );
    (Arc::new(wrapped), Some(ledger))
}

/// Names the index a key of this deployment belongs to.
struct KeyOwner(String);

impl oss_blob::usage::Classifier for KeyOwner {
    fn index_of<'a>(&self, key: &'a str) -> Option<&'a str> {
        oss_format::index_of_key(&self.0, key)
    }
}

/// `opener`, reading a bucket member's commit from its bucket when the
/// buckets switch is on.
pub(super) fn with_member_commits(
    opener: crate::writer::HeadOpener,
    buckets: Option<&Arc<crate::writer::BucketWiring>>,
) -> crate::writer::HeadOpener {
    match buckets {
        Some(buckets) => {
            let reader = Arc::clone(buckets);
            opener.with_member_commits(Arc::new(move |bucket, uuid| {
                reader.member_commit(bucket, uuid)
            }))
        }
        None => opener,
    }
}
