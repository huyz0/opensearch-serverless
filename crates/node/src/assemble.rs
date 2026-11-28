//! The composition root of a node with the reader and writer roles: their
//! services and routes on one router, and the timers, over one shared store.

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, PooledStore, Shared};
use oss_mem::{BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_meta::{DescriptorStore, PitClock, PointInTimeStore};
use oss_rest::{Router, Server, ServerConfig};
use oss_writer::bulk::BulkBudget;

use crate::bulk_route::{self, BulkService};
use crate::clock::now_millis;
use crate::doc_route::{self, DocService};
use crate::get_route::{self, GetService};
use crate::handlers::Registry;
use crate::health_route::{self, HealthService, HealthSource};
#[cfg(feature = "coordinator")]
use crate::index_delete::{self, DrainTimer};
#[cfg(feature = "coordinator")]
use crate::index_route;
use crate::pit_route::{self, PitService};
use crate::refresh_route::{self, RefreshService};
use crate::search::{self, SearchService};
use crate::tail_node::{self, TailError, TailNode, TailParts};
use crate::update_route::{self, UpdateService};
use crate::writer::{JanitorPass, JanitorTimer, PassLimits, PublishTimer, WriterRole};

pub use crate::assemble_config::Config;
pub use crate::assemble_error::AssembleError;

/// Buffers the store's bodies stream through, a node's worth.
const STORE_BUFFERS: usize = 16;

/// The reclaim drain, present when the node serves index admin.
#[cfg(feature = "coordinator")]
type Drain = Option<DrainTimer>;
#[cfg(not(feature = "coordinator"))]
type Drain = Option<()>;

pub use crate::assemble_writer::Writer;

/// A node built and not yet listening.
pub struct Assembled {
    pub(crate) router: Router,
    /// The search service every `_search` goes through.
    pub search: Arc<SearchService<Shared>>,
    /// The role the write routes write through.
    pub role: Arc<WriterRole>,
    /// Asynchronous search, when the setting is on: the service that answers
    /// its routes and the calls other nodes relay.
    pub async_search: Option<Arc<crate::async_search::Service>>,
    /// The bulk service a background writer indexes documents through.
    pub(crate) bulk: Arc<BulkService>,
    timer: PublishTimer,
    janitor: JanitorTimer,
    drain: Drain,
    /// The tasks of background by-query and reindex requests.
    #[cfg(feature = "coordinator")]
    pub tasks: Arc<crate::by_query::Tasks>,
    tail: Option<TailNode>,
    pub(crate) budget: MemoryBudget,
    /// What the page cache's records are charged, held for the node's life.
    cache_records: Option<oss_mem::Reservation>,
    pub(crate) search_queue: Option<Arc<crate::search_queue::QueueAdmission>>,
    /// Where lifecycle events go, for the plugins that subscribe.
    pub(crate) events: crate::lifecycle_bus::Bus,
    /// The sender alerting and ISM post through; none when notifications are off.
    pub notifier: Option<Arc<crate::notifications_route::Notifier>>,
}

/// A node listening.
pub struct Running {
    pub(crate) server: Server,
    /// The search service every `_search` goes through.
    pub search: Arc<SearchService<Shared>>,
    /// The role the write routes write through.
    pub role: Arc<WriterRole>,
    _timer: PublishTimer,
    _janitor: JanitorTimer,
    _drain: Drain,
    _tail: Option<TailNode>,
    _cache_records: Option<oss_mem::Reservation>,
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
    /// Every route registered: its method (none for a prefix route) and pattern.
    pub fn routes(&self) -> Vec<(Option<oss_rest::Method>, &'static str)> {
        self.router.routes()
    }

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

    /// Changes the refresh visibility of this node; `None` while the tail
    /// overlay is off.
    pub fn tail_visibility(&self) -> Option<tail_node::VisibilityHandle> {
        self.tail.as_ref().map(|t| t.visibility.clone())
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
            _cache_records: self.cache_records,
        })
    }
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
    let pooled = Arc::new(
        PooledStore::new(Shared::new(Arc::clone(&store)), buffers)
            .with_wait(config.store_buffer_wait),
    );
    let (cache, cache_records) = crate::page_cache_disk::build(config, budget)?;
    // ALLOC: startup only.
    let search = Arc::new(SearchService::new(
        pooled,
        cache,
        &config.base,
        &config.node_id,
        budget,
    ));
    crate::search_modules::install(&search, &store, &config.base, budget);
    crate::search::alias::install(&search, &store, &config.base, config.delete_mode)
        .map_err(AssembleError::Store)?;
    let mut publishing = config.publishing(Arc::clone(&store));
    #[cfg(feature = "coordinator")]
    let bucket_wiring = writer.buckets.clone();
    if let Some(buckets) = writer.buckets {
        publishing = publishing.with_buckets(buckets);
    }
    // ALLOC: startup only.
    let mut role = WriterRole::new(writer.committer, writer.routes)
        .with_publishing(publishing)
        .with_hibernation(config.hibernate_after_millis)
        .with_give_back(config.give_back_after_millis);
    role.configure(config, budget);
    if config.commit_logged.writes() {
        let _ = role.log_commits(Duration::from_millis(config.publish_tick_millis));
    }
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
    let new_pits = || {
        PointInTimeStore::new(Arc::clone(&store), &config.base, Arc::clone(&clock))
            .map_err(|e| AssembleError::Store(e.to_string()))
    };
    role.set_mapping_growth(&store, config)?;
    let (pits, janitor_pits, search_pits) = (new_pits()?, new_pits()?, new_pits()?);
    search.set_point_in_time(search_pits, Arc::clone(&clock), config.pit_buffer_bytes);
    crate::admin_assemble::search_pipelines(config, &store, &search)?;
    crate::admin_assemble::stored_templates(config, &store, &search)?;
    let descriptors = DescriptorStore::new(Arc::clone(&store), &config.base, config.delete_mode)
        .map_err(|e| AssembleError::Store(e.to_string()))?;
    let pit = Arc::new(PitService::new(
        pits,
        descriptors,
        Arc::clone(&clock),
        config.pit_buffer_bytes,
    ));
    let closing = Arc::clone(&search);
    pit.set_release_hook(Arc::new(move |id| {
        closing.close_view(id);
    }));
    let ingest = crate::admin_assemble::ingest_pipelines(config, &store)?;
    let script_source = crate::admin_assemble::script_source(&store, config, budget)?;
    let bulk = Arc::new(
        BulkService::new(
            Arc::clone(&role),
            BulkBudget::new(config.bulk_bytes, config.bulk_holders),
            config.bulk,
            config.address,
        )
        .with_ingest(ingest.clone())
        .with_scripts(Arc::clone(&script_source) as _),
    );
    let docs = Arc::new(DocService::new(Arc::clone(&role), config.address).with_ingest(ingest));
    let router =
        search::routes(Router::new(), Arc::clone(&search)).map_err(AssembleError::Route)?;
    let router = if config.knn {
        search::knn_api::routes(router, &search).map_err(AssembleError::Route)?
    } else {
        router
    };
    let router = bulk_route::routes(router, Arc::clone(&bulk)).map_err(AssembleError::Route)?;
    let router = doc_route::routes(router, docs).map_err(AssembleError::Route)?;
    let router = get_route::routes(router, Arc::new(GetService::new(Arc::clone(&role))))
        .map_err(AssembleError::Route)?;
    let router = update_route::routes(
        router,
        Arc::new(
            UpdateService::new(Arc::clone(&role), config.address)
                .with_scripts(Arc::clone(&script_source) as _),
        ),
    )
    .map_err(AssembleError::Route)?;
    let router = refresh_route::routes(router, Arc::new(RefreshService::new(Arc::clone(&role))))
        .map_err(AssembleError::Route)?;
    let router = pit_route::routes(router, pit).map_err(AssembleError::Route)?;
    let (router, _sec) =
        crate::admin_assemble::register(router, &store, config, budget, &script_source)?;
    let router = crate::admin_assemble::ad_routes(router, config, &store, &search)?;
    let (router, notifier) = crate::notifications_assemble::mount(router, config, &store, budget)?;
    let router = crate::sql_route::mount(router, config, &search, _sec.as_ref())?;
    let (router, async_search) =
        crate::async_search::mount(router, config, budget, Arc::clone(&search) as _)?;
    let bus = crate::lifecycle_bus::Bus::default();
    #[cfg(feature = "coordinator")]
    let (router, drain, tasks) = {
        use crate::index_assemble::{index_service, pin_policy, restore_routes, stream_routes};
        let pins = pin_policy(config, &store, budget, _sec)?;
        let indices = index_service(config, &store, &clock, bucket_wiring.as_ref(), pins, &bus)?;
        let aliases = crate::index_assemble::alias_service(config, &store, &clock)?;
        let router = crate::alias_route::routes(router, aliases).map_err(AssembleError::Route)?;
        let deletes = crate::index_assemble::delete_service(
            config,
            &store,
            &clock,
            bucket_wiring.as_ref(),
            &bus,
        )?;
        let (router, restores) = restore_routes(router, config, &store, &clock, &deletes, budget)?;
        let drain =
            DrainTimer::spawn(Arc::clone(&deletes), config.reclaim_every).with_restores(restores);
        let router = crate::index_assemble::settings_routes(router, config, &store, &role)?;
        let router = stream_routes(router, config, &store, &clock, &indices, &deletes, &search)?;
        let router = index_route::routes(router, indices).map_err(AssembleError::Route)?;
        let router = index_delete::routes(router, deletes).map_err(AssembleError::Route)?;
        let tasks = Arc::new(crate::by_query::Tasks::new(
            &config.node_id,
            budget,
            crate::by_query::now_millis(),
        ));
        let by_query = Arc::new(
            crate::by_query::ByQueryService::new(
                Arc::clone(&role),
                Arc::new(crate::by_query::SearchMatches::new(Arc::clone(&search))),
                BulkBudget::new(config.bulk_bytes, config.bulk_holders),
                config.address,
                Arc::clone(&tasks),
            )
            .with_scripts(Arc::clone(&script_source) as _),
        );
        let router = crate::by_query::routes(router, by_query).map_err(AssembleError::Route)?;
        (router, Some(drain), tasks)
    };
    #[cfg(not(feature = "coordinator"))]
    let drain: Option<()> = None;
    let modules =
        crate::plugins::configured_modules(&config.plugin_policy, &*store, &config.base, budget);
    // The plugins' own job registry, reserved from the plugin pool.
    let plugin_jobs = Arc::new(crate::plugins::PluginJobs::new(
        crate::jobs::Jobs::new(
            &config.node_id,
            budget,
            crate::jobs::Limits::default(),
            Arc::new(crate::clock::now_millis),
        ),
        Arc::new(crate::async_search::SearchRunner::new(
            Arc::clone(&search) as _
        )),
    ));
    let router = crate::plugins::install_configured(
        router,
        &config.plugin_policy,
        &bulk,
        plugin_jobs,
        &bus,
        &modules,
    )?;

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
        async_search,
        search_queue: None,
        events: bus,
        notifier,
        router,
        search,
        role,
        bulk,
        timer,
        janitor,
        drain,
        #[cfg(feature = "coordinator")]
        tasks,
        tail: None,
        budget: budget.clone(),
        cache_records,
    })
}
