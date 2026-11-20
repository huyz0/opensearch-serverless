//! The composition root of a node with the reader and writer roles: the
//! services each role serves, their routes on one router, and the publish
//! and janitor timers, over one shared store.

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, PooledStore, Shared};
use oss_format::MAX_POINT_IN_TIME_BYTES;
use oss_janitor::gc;
use oss_mem::{BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_meta::{DeleteMode, DescriptorStore, PitClock, PointInTimeStore};
use oss_rest::{RouteError, Router, Server, ServerConfig};
use oss_writer::bulk::{BulkBudget, StreamConfig};
use oss_writer::log::NodeLogCommitter;
use oss_writer::publish::lazy::Policy;

use crate::bulk_route::{self, BulkService};
use crate::doc_route::{self, DocService};
use crate::get_route::{self, GetService};
use crate::handlers::Registry;
use crate::health_route::{self, HealthService, HealthSource};
#[cfg(feature = "coordinator")]
use crate::index_delete::{self, DeleteService, DrainTimer, NoSnapshots};
#[cfg(feature = "coordinator")]
use crate::index_route::{self, IndexService};
use crate::pit_route::{self, PitService};
use crate::refresh_route::{self, RefreshService};
use crate::search::{self, SearchService};
use crate::tail_node::{self, TailError, TailNode, TailParts};
use crate::writer::{
    Incarnation, JanitorPass, JanitorTimer, PassLimits, PublishTimer, Publishing, Routes,
    ShardNames, ShardOpener, WriterRole,
};

/// Buffers the store's bodies stream through, a node's worth.
const STORE_BUFFERS: usize = 16;

/// How a node is sized and named.
#[derive(Clone, Debug)]
pub struct Config {
    /// The deployment's base in the store (empty, or ending in `/`).
    pub base: String,
    /// This node's id.
    pub node_id: String,
    /// The six bytes time-based ids are made with.
    pub address: [u8; 6],
    /// The bytes of `_bulk` bodies in flight, node wide.
    pub bulk_bytes: usize,
    /// The bulks that may hold a share at once.
    pub bulk_holders: usize,
    /// How a `_bulk` body streams.
    pub bulk: StreamConfig,
    /// The buffer a point-in-time request holds.
    pub pit_buffer_bytes: usize,
    /// How a point in time's register is deleted.
    pub delete_mode: DeleteMode,
    /// How often the publish timer runs a pass.
    pub publish_every: Duration,
    /// `serverless.wal.node_log.publish_max_age_millis`.
    pub publish_age_millis: u64,
    /// `serverless.publish.max_unpublished_operations`.
    pub publish_max_operations: u64,
    /// The write log's window, which a due time rounds up to.
    pub publish_tick_millis: u64,
    /// How often the janitor runs a pass.
    pub janitor_every: Duration,
    /// How a janitor sweep behaves; it starts as a dry run.
    pub sweep: gc::Config,
    /// How often this node drains the reclaim queue.
    pub reclaim_every: Duration,
    /// The most points in time that may be live for a janitor pass to run.
    pub janitor_max_views: usize,
}

impl Config {
    /// The defaults for node `node_id` under `base`.
    pub fn new(base: &str, node_id: &str, address: [u8; 6]) -> Self {
        Self {
            // ALLOC: two names, once a node.
            base: base.to_owned(),
            node_id: node_id.to_owned(),
            address,
            bulk_bytes: 64 << 20,
            bulk_holders: 4,
            bulk: StreamConfig::default(),
            pit_buffer_bytes: MAX_POINT_IN_TIME_BYTES,
            delete_mode: DeleteMode::Tombstone,
            publish_every: Duration::from_millis(250),
            publish_age_millis: 1_000,
            publish_max_operations: 10_000,
            publish_tick_millis: 250,
            janitor_every: Duration::from_secs(60),
            sweep: gc::Config {
                dry_run: true,
                minimum_unreferenced_millis: 60_000,
                max_watched: 100_000,
                max_pinned: 100_000,
            },
            reclaim_every: Duration::from_secs(60),
            janitor_max_views: 10_000,
        }
    }
}

/// The reclaim drain, present when the node serves index admin.
#[cfg(feature = "coordinator")]
type Drain = Option<DrainTimer>;
#[cfg(not(feature = "coordinator"))]
type Drain = Option<()>;

#[cfg(feature = "coordinator")]
fn delete_service(
    config: &Config,
    store: &Arc<dyn BlobStore>,
    clock: &PitClock,
) -> Result<Arc<DeleteService>, AssembleError> {
    DeleteService::new(
        store,
        &config.base,
        config.delete_mode,
        Arc::clone(clock),
        Arc::new(NoSnapshots),
        config.pit_buffer_bytes,
    )
    .map(Arc::new)
    .map_err(|e| AssembleError::Store(e.to_string()))
}

/// What the writer role is built on.
pub struct Writer {
    /// The node's one write log.
    pub committer: Arc<NodeLogCommitter>,
    /// Which indexes exist and who owns a shard this node does not hold.
    pub routes: Arc<dyn Routes>,
    /// What opens a shard a write names and the node does not hold yet.
    pub opener: Option<Arc<dyn ShardOpener>>,
    /// The names the committer's admission finds a shard's head by, shared
    /// with it; none, and the role keeps its own.
    pub names: Option<Arc<ShardNames>>,
    /// Confirms an open shard's index is still the incarnation it was
    /// opened for, at most once a window.
    pub incarnation: Option<(Arc<dyn Incarnation>, Duration)>,
}

/// Why a node was not assembled.
#[derive(Debug)]
pub enum AssembleError {
    /// The budget gives the `open_indexes` pool nothing, so no index could
    /// ever open.
    NoOpenIndexes,
    /// The page cache or the store's buffers could not be set up.
    Memory(String),
    /// A store refused the deployment's base.
    Store(String),
    /// A route is registered twice.
    Route(RouteError),
    /// The HTTP server did not start.
    Server(oss_rest::StartError),
}

impl std::fmt::Display for AssembleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssembleError::NoOpenIndexes => f.write_str("the open_indexes pool is empty"),
            AssembleError::Memory(e) => write!(f, "memory: {e}"),
            AssembleError::Store(e) => write!(f, "store: {e}"),
            AssembleError::Route(e) => write!(f, "route: {e:?}"),
            AssembleError::Server(e) => write!(f, "server: {e:?}"),
        }
    }
}

impl std::error::Error for AssembleError {}

/// A node built and not yet listening.
pub struct Assembled {
    router: Router,
    /// The search service every `_search` goes through.
    pub search: Arc<SearchService<Shared>>,
    /// The role the write routes write through.
    pub role: Arc<WriterRole>,
    timer: PublishTimer,
    janitor: JanitorTimer,
    drain: Drain,
    tail: Option<TailNode>,
    budget: MemoryBudget,
}

/// A node listening.
pub struct Running {
    server: Server,
    /// The search service every `_search` goes through.
    pub search: Arc<SearchService<Shared>>,
    /// The role the write routes write through.
    pub role: Arc<WriterRole>,
    _timer: PublishTimer,
    _janitor: JanitorTimer,
    _drain: Drain,
    _tail: Option<TailNode>,
}

impl Running {
    /// What the tail overlay holds, when it is on.
    pub fn tail(&self) -> Option<&TailNode> {
        self._tail.as_ref()
    }

    /// The address the node listens on.
    pub fn addr(&self) -> SocketAddr {
        self.server.local_addr()
    }
}

impl Assembled {
    /// Registers `GET /_cluster/health[/{index}]` and `GET /_cat/health` over
    /// `source`, answering as the cluster named `cluster`. The assembly does
    /// not make a source: it needs the node's membership, which the caller
    /// has once it has enrolled.
    ///
    /// # Errors
    ///
    /// [`AssembleError::Route`] when a health route is registered twice.
    pub fn with_health(
        mut self,
        source: Arc<dyn HealthSource>,
        cluster: &str,
    ) -> Result<Self, AssembleError> {
        let service = Arc::new(HealthService::new(source, cluster));
        self.router = health_route::routes(self.router, service).map_err(AssembleError::Route)?;
        Ok(self)
    }

    /// Turns the tail overlay on when `parts` say so, and gives the
    /// transport handlers for it; `None` when it is off or already on.
    ///
    /// # Errors
    ///
    /// [`TailError`] when a tail action is registered twice.
    pub fn wire_tail(&mut self, parts: TailParts) -> Result<Option<Registry>, TailError> {
        if self.tail.is_some() {
            return Ok(None);
        }
        let wired = tail_node::wire(&self.search, &self.role, self.budget.clone(), parts)?;
        Ok(wired.map(|(node, registry)| {
            self.tail = Some(node);
            registry
        }))
    }

    /// Starts serving on `listener`, the server's buffers charged to
    /// `budget`.
    ///
    /// # Errors
    ///
    /// [`AssembleError::Server`] when the server does not start.
    pub fn start(
        self,
        listener: TcpListener,
        config: ServerConfig,
        budget: &MemoryBudget,
    ) -> Result<Running, AssembleError> {
        let server =
            Server::start(listener, config, self.router, budget).map_err(AssembleError::Server)?;
        Ok(Running {
            server,
            search: self.search,
            role: self.role,
            _timer: self.timer,
            _janitor: self.janitor,
            _drain: self.drain,
            _tail: self.tail,
        })
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Builds a node's services over `store` and registers their routes.
///
/// # Errors
///
/// An [`AssembleError`] when the budget cannot hold the node, a store
/// refuses the base, or a route is registered twice.
pub fn assemble(
    config: &Config,
    store: Arc<dyn BlobStore>,
    budget: &MemoryBudget,
    writer: Writer,
) -> Result<Assembled, AssembleError> {
    if budget.limit(Pool::OpenIndexes) == 0 {
        return Err(AssembleError::NoOpenIndexes);
    }
    let buffers = BufferPool::new(budget, Pool::Transport, SMALL_BUFFER, STORE_BUFFERS)
        .map_err(|e| AssembleError::Memory(format!("{e:?}")))?;
    let pooled = Arc::new(PooledStore::new(Shared::new(Arc::clone(&store)), buffers));
    let cache = PageCache::new(PageCacheConfig::default())
        .map_err(|e| AssembleError::Memory(e.to_string()))?;
    // ALLOC: startup only.
    let search = Arc::new(SearchService::new(
        pooled,
        Arc::new(cache),
        &config.base,
        &config.node_id,
        budget,
    ));
    let publishing = Publishing::new(
        Arc::clone(&store),
        &config.base,
        &config.node_id,
        Policy::new(
            config.publish_age_millis,
            config.publish_max_operations,
            config.publish_tick_millis,
        ),
        Box::new(now_millis),
    );
    // ALLOC: startup only.
    let mut role = WriterRole::new(writer.committer, writer.routes).with_publishing(publishing);
    if let Some(opener) = writer.opener {
        role = role.with_opener(opener);
    }
    if let Some(names) = writer.names {
        role = role.with_names(names);
    }
    if let Some((check, window)) = writer.incarnation {
        role = role.with_incarnation(check, window);
    }
    let role = Arc::new(role);
    let clock: PitClock = Arc::new(now_millis);
    let pits = PointInTimeStore::new(Arc::clone(&store), &config.base, Arc::clone(&clock))
        .map_err(|e| AssembleError::Store(e.to_string()))?;
    let janitor_pits = PointInTimeStore::new(Arc::clone(&store), &config.base, Arc::clone(&clock))
        .map_err(|e| AssembleError::Store(e.to_string()))?;
    let search_pits = PointInTimeStore::new(Arc::clone(&store), &config.base, Arc::clone(&clock))
        .map_err(|e| AssembleError::Store(e.to_string()))?;
    search.set_point_in_time(search_pits, Arc::clone(&clock), config.pit_buffer_bytes);
    let descriptors = DescriptorStore::new(Arc::clone(&store), &config.base, config.delete_mode)
        .map_err(|e| AssembleError::Store(e.to_string()))?;
    let pit = Arc::new(PitService::new(
        pits,
        descriptors,
        Arc::clone(&clock),
        config.pit_buffer_bytes,
    ));
    let bulk = Arc::new(BulkService::new(
        Arc::clone(&role),
        BulkBudget::new(config.bulk_bytes, config.bulk_holders),
        config.bulk,
        config.address,
    ));
    let docs = Arc::new(DocService::new(Arc::clone(&role), config.address));
    let router =
        search::routes(Router::new(), Arc::clone(&search)).map_err(AssembleError::Route)?;
    let router = bulk_route::routes(router, bulk).map_err(AssembleError::Route)?;
    let router = doc_route::routes(router, docs).map_err(AssembleError::Route)?;
    let router = get_route::routes(router, Arc::new(GetService::new(Arc::clone(&role))))
        .map_err(AssembleError::Route)?;
    let router = refresh_route::routes(router, Arc::new(RefreshService::new(Arc::clone(&role))))
        .map_err(AssembleError::Route)?;
    let router = pit_route::routes(router, pit).map_err(AssembleError::Route)?;
    let router = crate::admin_assemble::register(router, &store, config)?;
    #[cfg(feature = "coordinator")]
    let (router, drain) = {
        let descriptors =
            DescriptorStore::new(Arc::clone(&store), &config.base, config.delete_mode)
                .map_err(|e| AssembleError::Store(e.to_string()))?;
        let indices = Arc::new(IndexService::new(
            descriptors,
            Arc::clone(&clock),
            config.pit_buffer_bytes,
        ));
        let router = index_route::routes(router, indices).map_err(AssembleError::Route)?;
        let deletes = delete_service(config, &store, &clock)?;
        let drain = DrainTimer::spawn(Arc::clone(&deletes), config.reclaim_every);
        let router = index_delete::routes(router, deletes).map_err(AssembleError::Route)?;
        (router, Some(drain))
    };
    #[cfg(not(feature = "coordinator"))]
    let drain: Option<()> = None;

    let timer = PublishTimer::spawn(Arc::clone(&role), config.publish_every);
    let pass = Arc::new(JanitorPass::new(
        Arc::clone(&store),
        &config.base,
        Arc::clone(&role),
        janitor_pits,
        budget,
        PassLimits {
            buffer_bytes: config.pit_buffer_bytes,
            max_views: config.janitor_max_views,
        },
        Box::new(|| i64::try_from(now_millis()).unwrap_or(i64::MAX)),
    ));
    let sweep = config.sweep;
    let janitor = JanitorTimer::spawn(pass, config.janitor_every, Box::new(move || sweep));
    Ok(Assembled {
        router,
        search,
        role,
        timer,
        janitor,
        drain,
        tail: None,
        budget: budget.clone(),
    })
}
