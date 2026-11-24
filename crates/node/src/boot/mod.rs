//! A node started from configuration: its store, memory, lease, write log
//! and roles, assembled and listening.
//!
//! Spec: docs/specs/node-boot/SPEC.md. The lease is liveness only: the shard
//! head's conditional write is what fences a writer, so a late renewal can
//! cost a handover and never a write.

mod config;
mod error;
mod follow;
mod join;
mod renew;
mod retire;
mod support;
mod tail;
mod transport;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_cluster::heads::ShardHeadStore;
use oss_cluster::transport_secret::{CachedTransportSecrets, TransportSecretStore};
use oss_cluster::{
    EarlyRevoke, LeaseClock, LeaseConfig, LivenessOracle, Membership, SystemClock, TcpProbe,
};
use oss_mem::MemoryBudget;
use oss_meta::{ChangeStripes, DeleteMode, DescriptorStore};
use oss_rest::ServerConfig;
use oss_transport::actions::read::GET_ACTION;
use oss_transport::actions::write::BULK_ACTION;
use oss_transport::auth::{AuthConfig, Authenticator};
use oss_transport::client::{Client, ClientConfig};
use oss_transport::server::{Server as TransportServer, ServerConfig as TransportConfig};
use oss_writer::log::{CommitterConfig, LeaseKeeper, NodeLogCommitter, NodeLogStore};

use crate::assemble::{self, Config, Running, Writer};
use crate::bulk_handler::BulkHandler;
use crate::cluster_owner::ClusterRoutes;
use crate::descriptor_routes::DescriptorRoutes;
use crate::forward::BulkForwarder;
use crate::handlers::Registry;
use crate::health_route::StoreHealth;
use crate::index_cache::IndexCache;
use crate::read_forward::{GetForwarder, GetHandler};
use crate::usage_route;
use crate::writer::{
    DeadLogParts, DeadLogPass, DeadMailbox, DeadSnapshot, DescriptorIncarnation, HeadAdmission,
    HeadOpener, LiveLeases, MembershipLeaseWrite, OrphanTimer, ShardNames, Takeover,
};

pub use config::{MIN_MEMORY_BYTES, NodeConfig, StoreChoice};
pub use error::BootError;
use renew::{Cadence, Renewal};
use support::{Held, address_bytes, earlier_epochs, store_of, usage_of};

/// The roles a node's lease names.
const ROLES: [&str; 2] = ["ingest", "search"];

/// The most members a node's membership view holds.
const MAX_MEMBERS: usize = 1024;

/// The buffer a descriptor, a head or a lease is read through.
const READ_BUFFER: usize = 1 << 20;

/// The bytes a takeover's replay buffers before it commits locally.
const REPLAY_BUFFER: usize = 16_777_216;

/// The most request nonces the transport remembers.
const REPLAY_CAPACITY: usize = 65_536;

/// How long a held shard's index incarnation is trusted before it is read again.
const INCARNATION_WINDOW: Duration = Duration::from_secs(600);

/// A node running: dropping it stops its lease renewal, its servers, and
/// releases its lease.
pub struct Node {
    running: Option<Running>,
    transport: Option<TransportServer>,
    renewal: Option<Renewal>,
    dead_logs: Option<Renewal>,
    /// The orphan-shard sweep, present when its budget is above 0.
    orphan_sweep: Option<OrphanTimer>,
    following: Option<follow::Following>,
    membership: Arc<Membership>,
    node_id: String,
    /// Whether dropping the node leaves its lease to lapse, as a killed
    /// process does.
    crashed: bool,
}

impl Node {
    /// Stops the node as a killed process stops: its servers and its
    /// renewal end, and its lease is not released, so it lapses in one
    /// lifetime and the other nodes see it die.
    pub fn crash(mut self) {
        self.crashed = true;
    }

    /// The address the node's HTTP server listens on.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.running.as_ref().map_or_else(
            || std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
            Running::addr,
        )
    }

    /// Whether the tail overlay is on: writers push to this node's reader
    /// side and its searches see what they pushed.
    pub fn tail_on(&self) -> bool {
        self.running.as_ref().is_some_and(|r| r.tail().is_some())
    }

    /// The refresh visibility the node is in, which follows the cluster
    /// register; `None` while the tail overlay is off.
    pub fn refresh_visibility(&self) -> Option<crate::writer::RefreshVisibility> {
        self.following.as_ref().map(follow::Following::mode)
    }

    /// The address the node's transport server listens on.
    pub fn transport_addr(&self) -> std::net::SocketAddr {
        self.transport.as_ref().map_or_else(
            || std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
            TransportServer::address,
        )
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        // The renewal stops first, so it cannot write the lease back.
        drop(self.renewal.take());
        drop(self.dead_logs.take());
        drop(self.orphan_sweep.take());
        drop(self.following.take());
        drop(self.running.take());
        drop(self.transport.take());
        // Best effort: an unreleased lease lapses in one lifetime.
        if !self.crashed {
            let _ = self.membership.release(&self.node_id);
        }
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
    let (store, usage) = usage_of(store_of(&config.store)?, config);
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

    // The epochs an earlier process of this node left, read from its own
    // lease register whether or not it is still listed or has expired: this
    // process writes above them and names them in every lease, so a
    // successor still fences them.
    let inherited = earlier_epochs(store.as_ref(), &config.base, &config.node_id)?;
    let first_epoch = inherited
        .iter()
        .map(|e| u64::try_from(e.epoch).unwrap_or(0))
        .max()
        .map_or(1, |m| m.saturating_add(1));

    // The transport listens before the lease names its address; what it
    // answers is filled in once the roles exist.
    let secrets = TransportSecretStore::new(Arc::clone(&store), &config.base, Arc::clone(&clock))
        .map_err(cluster)?;
    let auth = Arc::new(Authenticator::new(
        AuthConfig {
            identity: oss_transport::auth::identity(&config.node_id, &ephemeral),
            window_millis: config.lease_ttl_millis,
            replay_capacity: REPLAY_CAPACITY,
        },
        Arc::new(CachedTransportSecrets::new(
            secrets,
            config.lease_ttl_millis,
        )),
        {
            let clock = Arc::clone(&clock);
            Arc::new(move || clock.now_millis())
        },
    ));
    let late = transport::Late::default();
    let late_slot = late.slot();
    let late_get = transport::Late::default();
    let late_get_slot = late_get.slot();
    let late_push = transport::Late::default();
    let late_push_slot = late_push.slot();
    let mut registry = Registry::new(Arc::clone(&auth));
    registry
        .add(oss_transport::actions::push::PUSH_ACTION, late_push)
        .map_err(|e| BootError::Cluster(e.to_string()))?;
    registry
        .add(BULK_ACTION, late)
        .map_err(|e| BootError::Cluster(e.to_string()))?;
    registry
        .add(GET_ACTION, late_get)
        .map_err(|e| BootError::Cluster(e.to_string()))?;
    join::register(&mut registry, &membership).map_err(|e| BootError::Cluster(e.to_string()))?;
    let mut handlers = registry.into_handlers();
    let tail_slots = tail::Slots::list(&mut handlers);
    let transport_server = TransportServer::bind(
        config.transport,
        TransportConfig::new(
            transport::describe(&config.node_id, &ephemeral, config.transport),
            &config.cluster,
            MAX_MEMBERS,
        ),
        budget.clone(),
        handlers,
    )
    .map_err(|e| BootError::Bind(format!("{}: {e}", config.transport)))?;
    let advertise = config
        .advertise
        .clone()
        .unwrap_or_else(|| transport_server.address().to_string());
    let lease = support::own_lease(config, &ephemeral, &advertise, &inherited);

    let oracle = Arc::new(LivenessOracle::new(Arc::clone(&membership)));
    let descriptors = Arc::new(
        DescriptorStore::new(Arc::clone(&store), &config.base, DeleteMode::Tombstone)
            .map_err(cluster)?,
    );
    let buckets = support::bucket_wiring(config, &store, &descriptors, &clock)?;
    let heads = support::head_store(config, &store, &clock, &oracle, buckets.as_ref())?;
    let names = Arc::new(ShardNames::default());
    let log = NodeLogStore::new(Arc::clone(&store), &config.base, &config.node_id)
        .map(|log| support::shared_in(log, usage.as_ref()))
        .map_err(cluster)?;
    let lease_for_enrolment = lease.clone();
    // What the lease tells peers this node holds; the renewal thread keeps it.
    let load = Arc::new(crate::writer::LoadReport::default());
    let keeper = Arc::new(LeaseKeeper::new(
        Box::new(
            MembershipLeaseWrite::new(Arc::clone(&membership), lease).with_load(Arc::clone(&load)),
        ),
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
    let takeover = Takeover::from_leases(
        Arc::clone(&store),
        &config.base,
        &config.node_id,
        i64::try_from(first_epoch).unwrap_or(i64::MAX),
        REPLAY_BUFFER,
    );
    // The take pass reads a dead node's log through the same predecessors the
    // opener uses, so a log is fenced once however a shard is taken.
    let predecessors = Arc::clone(&takeover.predecessors);
    let opener = HeadOpener::new(
        Arc::clone(&heads),
        Arc::clone(&descriptors),
        Arc::clone(&store),
        &config.base,
        &config.node_id,
        &ephemeral,
        READ_BUFFER,
    )
    .with_takeover(takeover);
    let opener = support::with_member_commits(opener, buckets.as_ref());
    // One cache of the indexes this node reads, shared by every route that
    // resolves one; its confirmations are the change stripes every node
    // records its descriptor changes in.
    let index_cache = if config.index_confirm_millis == 0 {
        None
    } else {
        Some(Arc::new(IndexCache::new(
            ChangeStripes::new(Arc::clone(&store), &config.base).map_err(cluster)?,
            Duration::from_millis(config.index_confirm_millis),
            Box::new(std::time::Instant::now),
        )))
    };
    let descriptor_routes = || -> Result<DescriptorRoutes, BootError> {
        let routes = DescriptorRoutes::new(
            DescriptorStore::new(Arc::clone(&store), &config.base, DeleteMode::Tombstone)
                .map_err(cluster)?,
            READ_BUFFER,
        );
        Ok(match &index_cache {
            Some(cache) => routes.with_cache(Arc::clone(cache)),
            None => routes,
        })
    };
    // A write another node owns goes to it over the transport. Early
    // revoke stays off: it is a setting this node does not read yet.
    let client = Arc::new(Client::new(ClientConfig::default(), budget.clone()));
    let forwarder = BulkForwarder::new(
        Arc::clone(&client),
        Arc::clone(&auth),
        Held(Arc::clone(&membership)),
        Arc::new(EarlyRevoke::new(
            Arc::clone(&membership),
            Arc::new(TcpProbe::default()),
            false,
        )),
        Arc::clone(&clock),
    );
    let reads = GetForwarder::new(
        Arc::clone(&client),
        Arc::clone(&auth),
        Held(Arc::clone(&membership)),
    );
    let routes = ClusterRoutes::new(
        descriptor_routes()?,
        Held(Arc::clone(&membership)),
        forwarder,
        &config.node_id,
    )
    .with_reads(Arc::new(reads));
    let writer = Writer {
        committer: Arc::clone(&committer),
        routes: Arc::new(routes),
        opener: Some(Arc::new(opener)),
        names: Some(Arc::clone(&names)),
        incarnation: Some((
            Arc::new(DescriptorIncarnation::new(Arc::clone(&descriptors))),
            INCARNATION_WINDOW,
        )),
        buckets: buckets.clone(),
    };
    let mut node_config = Config::new(
        &config.base,
        &config.node_id,
        address_bytes(&config.node_id),
    );
    config::write_limit::apply(config, &mut node_config);
    support::apply_janitor_and_hibernation(&mut node_config, config);
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
    let mut assembled = assemble::assemble(&node_config, Arc::clone(&store), &budget, writer)
        .and_then(|a| a.with_health(health, &config.cluster))
        .and_then(|a| usage_route::attach(a, usage, config, &clock))
        .map_err(BootError::Assemble)?;
    if let Some(buckets) = &buckets {
        buckets.register_open(&config.node_id, Arc::clone(&assembled.role));
    }
    tail::wire(
        &mut assembled,
        config,
        &tail_slots,
        (Arc::clone(&client), Arc::clone(&auth)),
        &membership,
        Arc::clone(&clock),
        Arc::clone(&store),
    )?;
    let following = follow::start(assembled.tail_visibility(), Arc::clone(&store), config)?;
    let handler: Arc<dyn oss_transport::server::Handler> = Arc::new(BulkHandler::new(
        Arc::clone(&assembled.role),
        Arc::new(descriptor_routes()?),
        &config.node_id,
    ));
    late_slot.set(handler).ok();
    let get_handler: Arc<dyn oss_transport::server::Handler> = Arc::new(GetHandler::new(
        Arc::clone(&assembled.role),
        &config.node_id,
    ));
    late_get_slot.set(get_handler).ok();
    let push_handler: Arc<dyn oss_transport::server::Handler> =
        Arc::new(crate::push::PushHandler::new(Arc::clone(&assembled.search)));
    late_push_slot.set(push_handler).ok();
    support::wire_commit_push(&assembled, &client, &auth, &membership, config)?;
    // Enrolled last: a peer that sees the lease can reach everything it names.
    membership.renew(&lease_for_enrolment).map_err(cluster)?;
    join::announce(&membership, client, &auth, &config.node_id, &ephemeral)
        .map_err(|e| BootError::Cluster(format!("lease announce thread: {e}")))?;
    let listener = TcpListener::bind(config.http)
        .map_err(|e| BootError::Bind(format!("{}: {e}", config.http)))?;
    let role = Arc::clone(&assembled.role);
    let fleet = support::FleetTick::new(&role, load, &clock, config);
    let running = assembled
        .start(listener, ServerConfig::default(), &budget)
        .map_err(BootError::Assemble)?;
    // Dead nodes naming log epochs come from each renewal's refresh; a thread of its own collects them.
    let mailbox = Arc::new(DeadMailbox::default());
    let posting = Arc::clone(&mailbox);
    let renewing = Arc::clone(&membership);
    let proactive = support::proactive_takeover(config, role, Arc::clone(&predecessors));
    let taking_clock = Arc::clone(&clock);
    let mut cadence = Cadence::new(config.renew_every(), config.refresh_every(), Instant::now());
    let renewal = Renewal::spawn(cadence.tick_every(), move || {
        let now = Instant::now();
        if cadence.renew_due(now) {
            fleet.renewing();
            let _ = keeper.renew(&committer);
        }
        if cadence.refresh_due(now)
            && let Ok(refreshed) = renewing.refresh()
        {
            cadence.refreshed(now);
            proactive.on_refresh(&refreshed, taking_clock.now_millis());
            fleet.refreshed(&renewing);
            posting.post(DeadSnapshot {
                live: refreshed.live,
                dead: refreshed.dead_with_epochs,
            });
        }
    })
    .map_err(|e| BootError::Cluster(format!("lease renewal thread: {e}")))?;
    let collecting = DeadLogPass::new(
        DeadLogParts {
            membership: Arc::clone(&membership),
            heads: Arc::clone(&heads),
            descriptors: Arc::clone(&descriptors),
            store: Arc::clone(&store),
            base: config.base.clone(),
            node_id: config.node_id.clone(),
            term: i64::try_from(first_epoch).unwrap_or(i64::MAX),
            budget: budget.clone(),
            buffer_bytes: READ_BUFFER,
            first_wait_millis: i64::try_from(node_config.janitor_every.as_millis())
                .unwrap_or(i64::MAX),
            predecessors: Some(predecessors),
        },
        {
            let clock = Arc::clone(&clock);
            Box::new(move || clock.now_millis())
        },
    );
    let dry_run = node_config.sweep.dry_run;
    let live_members = Arc::new(LiveLeases::default());
    let orphan_sweep = support::orphan_sweep(
        config,
        &store,
        &budget,
        Arc::clone(&clock),
        Arc::clone(&live_members),
        dry_run,
    );
    let tombstones = support::tombstone_pass(config, &store, &budget, Arc::clone(&clock));
    let dead_logs = Renewal::spawn_named("dead-log-pass", node_config.janitor_every, move || {
        if let Some(snapshot) = mailbox.take() {
            live_members.set(Arc::clone(&snapshot.live));
            let _ = collecting.run(&snapshot, dry_run);
            let _ = tombstones.run(&snapshot.live);
        }
    })
    .map_err(|e| BootError::Cluster(format!("dead log thread: {e}")))?;
    Ok(Node {
        transport: Some(transport_server),
        running: Some(running),
        renewal: Some(renewal),
        dead_logs: Some(dead_logs),
        orphan_sweep,
        following,
        membership,
        node_id: config.node_id.clone(),
        crashed: false,
    })
}
