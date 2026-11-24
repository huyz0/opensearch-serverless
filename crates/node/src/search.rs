//! `GET|POST /{index}/_search` on a reader: the REST handler over
//! `oss-reader`.
//!
//! Ported from `RestSearchAction.prepareRequest`
//! (server/src/main/java/org/opensearch/rest/action/search/RestSearchAction.java)
//! for the `size` and `from` parameters, and the index lookup's
//! `IndexNotFoundException` (server/src/main/java/org/opensearch/index/IndexNotFoundException.java),
//! at OpenSearch 3.10.0. The body streams into the reader's token recorder
//! a chunk at a time, charged to the request-body pool as it grows. An
//! index is opened on its first search, with two GETs, and kept open: this
//! node never polls the object store for an index, so it sees a newer
//! commit only once it is told of one. What an open index keeps in the heap
//! is charged to the `open_indexes` pool before it is opened; when the pool
//! is full the least recently searched index is closed
//! ([ADR 0014](../../../docs/adr/0014-open-indexes-pool.md)).

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::open_indexes::{OpenSet, lock};
use lucene_analysis::Analyzer;
use lucene_store::page_cache::PageCache;
use oss_blob::{BlobStore, PooledStore};
use oss_mem::{MemoryBudget, Pool, Reservation};
use oss_reader::OpenShard;
use oss_reader::commit_push::{NewestSource, PushedCommits, Stage};
use oss_reader::dsl::{DslError, MAX_RECORDED_BYTES, Recorder, SearchSource, source};
use oss_reader::failure::{SearchError, ShardFailure};
use oss_reader::overlay::OverlaySource;
use oss_reader::response::write_response;
use oss_reader::search::{hits_bytes, search};
use oss_rest::{JSON, Length, Method, Request, Response, RestError, RouteError, Router};

/// The query parameters `_search` takes.
pub const SEARCH_PARAMS: &[&str] = &["size", "from"];

/// An open index and its charge to the `open_indexes` pool, released when
/// the last search holding it finishes.
pub(crate) struct Held<S: BlobStore + 'static> {
    pub(crate) shard: OpenShard<S>,
    /// The shard with an overlay on top, for the overlay it was made from.
    view: Mutex<Option<overlay::OverlayView<S>>>,
    /// Where the commit it is open on sits among its writer's.
    pub(crate) stage: Stage,
    /// The index's name.
    pub(crate) index: String,
    /// Which shard of the index this is.
    pub(crate) shard_no: u32,
    charge: Reservation,
    /// Told when this engine lets go, so it can drop what it kept for it.
    source: Option<Arc<dyn OverlaySource>>,
    epoch: Instant,
}

/// How many shards' pushed commits a node keeps for the opens that follow.
const PUSHED_SHARDS: usize = 4096;

/// The most bytes of pushed commits a node keeps.
const PUSHED_BYTES: usize = 4_194_304;

/// Serves searches of the indexes under one deployment base.
pub struct SearchService<S: BlobStore + 'static> {
    store: Arc<PooledStore<S>>,
    cache: Arc<PageCache>,
    base: String,
    node_id: String,
    budget: MemoryBudget,
    pub(crate) open: Mutex<OpenSet<Held<S>>>,
    /// Commits pushed for open indexes that fell behind, for the open that
    /// follows.
    pub(crate) pushed: Mutex<PushedCommits>,
    /// What the keep time of a pushed commit counts from.
    pub(crate) epoch: Instant,
    /// What asks a staging writer for its newest commit on a cold open.
    newest: OnceLock<Arc<dyn NewestSource>>,
    /// Where a search finds the overlay of the shard it serves; none, and a
    /// search is answered from the commit.
    overlay: OnceLock<Arc<dyn OverlaySource>>,
    /// Whether the shard a search serves holds what its writer answered as
    /// refreshed; none, and it is taken to.
    fresh: OnceLock<Arc<crate::tail_fresh::TailFresh>>,
    /// The layers a setting resolves through above an index's own.
    layers: OnceLock<open_prefetch::Layered>,
    /// Where a search of a point in time reads its view; none, and it is
    /// refused.
    pits: OnceLock<frozen::PitAccess>,
    /// The gate that orders searches under pressure, with what names a
    /// search's tenant and level; none, and every search runs at once.
    slots: OnceLock<crate::search_slots::Gated>,
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// A service reading `store` under `base`, through `cache`, reporting
    /// shard failures as node `node_id`, charging open indexes to
    /// `budget`'s `open_indexes` pool.
    pub fn new(
        store: Arc<PooledStore<S>>,
        cache: Arc<PageCache>,
        base: &str,
        node_id: &str,
        budget: &MemoryBudget,
    ) -> Self {
        Self {
            store,
            cache,
            // ALLOC: startup only.
            base: base.to_owned(),
            // ALLOC: startup only.
            node_id: node_id.to_owned(),
            budget: budget.clone(),
            open: Mutex::default(),
            pushed: Mutex::new(PushedCommits::new(PUSHED_SHARDS, PUSHED_BYTES)),
            epoch: Instant::now(),
            newest: OnceLock::new(),
            overlay: OnceLock::new(),
            fresh: OnceLock::new(),
            layers: OnceLock::new(),
            pits: OnceLock::new(),
            slots: OnceLock::new(),
        }
    }

    /// Installs the search gate. False when one is installed already.
    pub fn set_search_slots(&self, slots: crate::search_slots::Gated) -> bool {
        self.slots.set(slots).is_ok()
    }

    /// Installs what a cold open asks for a staging writer's newest commit.
    /// False when one is installed already.
    pub fn set_newest_source(&self, source: Arc<dyn NewestSource>) -> bool {
        self.newest.set(source).is_ok()
    }

    /// How many indexes are open.
    pub fn open_indexes(&self) -> usize {
        self.lock().len()
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, OpenSet<Held<S>>> {
        lock(&self.open)
    }

    /// Milliseconds since this service started: what a pushed commit's keep
    /// time counts.
    pub(crate) fn now_millis(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Handles one `_search`.
    pub fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        // Held to the end of the search, then the permit, if any, goes back.
        let _admitted = self.slots.get().and_then(|slots| {
            let index = req.path_param("index")?;
            slots.admit(&index)
        });
        self.handle_admitted(req, resp)
    }

    fn handle_admitted(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let started = Instant::now();
        let source = match read_source(req) {
            Ok(source) => source,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        let Some(index) = req.path_param("index") else {
            return match self.handle_pit(req, resp, &source, started) {
                Ok(()) => Ok(()),
                Err(Answer::Rest(e)) => Err(e),
                Err(Answer::Search(e)) => send(resp, &e),
            };
        };
        if source.pit.is_some() {
            return Err(RestError::illegal_argument(
                "a search of a point in time names no index: the view holds it",
            ));
        }
        #[cfg(feature = "coordinator")]
        match self.shard_count(&index) {
            Ok(shards) if shards > 1 => {
                return self.handle_sharded(resp, &index, shards, &source, started);
            }
            Ok(_) => {}
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        }
        let held = match self.shard(&index) {
            Ok(held) => held,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        let (held, view) = match self.fresh_held(held, true) {
            Ok(opened) => opened,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        let shard = view.as_deref().unwrap_or(&held.shard);
        req.body().reserve(hits_bytes(&source, shard))?;
        let analyzer = Analyzer::standard(None);
        let hits = match search(shard, &index, &source, &analyzer) {
            Ok(hits) => hits,
            Err(e) => {
                let shard_failure = ShardFailure {
                    // ALLOC: error path; the index name.
                    index: index.clone().into_owned(),
                    shard: 0,
                    // ALLOC: error path; the node id.
                    node: self.node_id.clone(),
                };
                return send(
                    resp,
                    &SearchError::on_shard(e.status, e.failure, shard_failure),
                );
            }
        };
        let took = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut body = resp.start(200, Some(JSON), Length::Chunked, &[])?;
        let pool = req.body();
        let mut reserve = |n| {
            pool.reserve(n)
                // ALLOC: error path; one box.
                .map_err(|_| std::io::Error::other("the request-body pool is full"))
        };
        write_response(
            &mut body,
            shard,
            &index,
            &self.node_id,
            &hits,
            took,
            &mut reserve,
        )?;
        body.flush()?;
        body.finish()
    }
}

/// What a request gets instead of hits.
enum Answer {
    /// An error the HTTP edge answers, such as a full request-body pool.
    Rest(RestError),
    /// A search error body.
    Search(Box<SearchError>),
}

fn bad_request(e: DslError) -> Answer {
    // ALLOC: error path; one box.
    Answer::Search(Box::new(SearchError::request(e.status(), e.into_failure())))
}

/// The body streamed through the recorder, its growth reserved first.
fn record_body(req: &mut Request<'_>) -> Result<Recorder, Answer> {
    let mut rec = Recorder::new(MAX_RECORDED_BYTES);
    let mut tokenizer = source::tokenizer();
    let body = req.body();
    loop {
        // What the recording may grow to is reserved before it grows.
        let left = body
            .declared_length()
            .map(|n| n.saturating_sub(body.bytes_read()));
        let chunk = left.map_or(body.max_chunk(), |n| {
            usize::try_from(n).map_or(body.max_chunk(), |n| n.min(body.max_chunk()))
        });
        let more = rec.bytes_after(chunk).saturating_sub(body.reserved());
        if more > 0 {
            body.reserve(more).map_err(Answer::Rest)?;
        }
        let Some(chunk) = body.next_chunk().map_err(Answer::Rest)? else {
            break;
        };
        tokenizer.feed(chunk, &mut rec).map_err(bad_request)?;
    }
    tokenizer.finish(&mut rec).map_err(bad_request)?;
    Ok(rec)
}

/// The body, streamed through the recorder, and the URL parameters.
fn read_source(req: &mut Request<'_>) -> Result<SearchSource, Answer> {
    let rec = record_body(req)?;
    let mut source = SearchSource::parse(&rec).map_err(bad_request)?;
    let from = req.param("from");
    let size = req.param("size");
    source
        .apply_params(from.as_deref(), size.as_deref())
        .map_err(bad_request)?;
    Ok(source)
}

fn search_error(e: SearchError) -> Answer {
    // ALLOC: error path; one box.
    Answer::Search(Box::new(e))
}

fn send(resp: &mut Response<'_>, e: &SearchError) -> Result<(), RestError> {
    resp.send(e.status, JSON, &e.to_body())
}

/// The routes of a reader: `_search` on one index, by GET and POST.
pub fn routes<S: BlobStore + 'static>(
    router: Router,
    service: Arc<SearchService<S>>,
) -> Result<Router, RouteError> {
    let get = Arc::clone(&service);
    let any_get = Arc::clone(&service);
    let any_post = Arc::clone(&service);
    let counting = Arc::clone(&service);
    let caps = Arc::clone(&service);
    let multi = Arc::clone(&service);
    let analysis = Arc::clone(&service);
    let validating = Arc::clone(&service);
    let templating = Arc::clone(&service);
    let ranking = Arc::clone(&service);
    router
        .route(
            Method::Get,
            "/_search",
            SEARCH_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| any_get.handle(req, resp),
        )?
        .route(
            Method::Post,
            "/_search",
            SEARCH_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| any_post.handle(req, resp),
        )?
        .route(
            Method::Get,
            "/{index}/_search",
            SEARCH_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| get.handle(req, resp),
        )?
        .route(
            Method::Post,
            "/{index}/_search",
            SEARCH_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp),
        )
        .and_then(|router| count::routes(router, counting))
        .and_then(|router| field_caps::routes(router, caps))
        .and_then(|router| msearch::routes(router, multi))
        .and_then(|router| analyze::routes(router, analysis))
        .and_then(|router| validate::routes(router, validating))
        .and_then(|router| template::routes(router, templating))
        .and_then(|router| rank_eval::routes(router, ranking))
}

#[path = "search_analyze.rs"]
mod analyze;
#[path = "search_count.rs"]
mod count;
#[path = "search_field_caps.rs"]
mod field_caps;
#[path = "search_frozen.rs"]
mod frozen;
#[path = "search_msearch.rs"]
mod msearch;
#[path = "search_open.rs"]
pub(crate) mod open_shard;
#[path = "search_rank_eval.rs"]
mod rank_eval;
#[path = "search_shard.rs"]
mod shard_query;
#[path = "search_template.rs"]
mod template;
#[path = "search_validate.rs"]
mod validate;
pub use shard_query::ShardQuery;
#[cfg(feature = "coordinator")]
#[path = "coordinate.rs"]
mod coordinate;
#[cfg(feature = "coordinator")]
pub use coordinate::ShardError;
#[path = "open_prefetch.rs"]
pub mod open_prefetch;
#[path = "search_overlay.rs"]
mod overlay;

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;

/// Why a reader node did not start.
#[derive(Debug)]
pub enum StartError {
    /// The page cache could not be set up.
    Cache(String),
    /// The budget gives the `open_indexes` pool nothing, so no index could
    /// ever open.
    NoOpenIndexes,
    /// A route could not be registered.
    Route(RouteError),
    /// The HTTP server did not start.
    Server(oss_rest::StartError),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Cache(e) => write!(f, "page cache: {e}"),
            StartError::NoOpenIndexes => f.write_str("the open_indexes pool is empty"),
            StartError::Route(e) => write!(f, "route: {e:?}"),
            StartError::Server(e) => write!(f, "server: {e:?}"),
        }
    }
}

impl std::error::Error for StartError {}

/// Starts a reader node: `_search` over the indexes under `base` in
/// `store`, served on `listener`, every server buffer and open index charged
/// to `budget`.
pub fn start_reader<S: BlobStore + 'static>(
    listener: std::net::TcpListener,
    config: oss_rest::ServerConfig,
    store: Arc<PooledStore<S>>,
    base: &str,
    node_id: &str,
    budget: &oss_mem::MemoryBudget,
) -> Result<(oss_rest::Server, Arc<SearchService<S>>), StartError> {
    if budget.limit(Pool::OpenIndexes) == 0 {
        return Err(StartError::NoOpenIndexes);
    }
    let cache = PageCache::new(lucene_store::page_cache::PageCacheConfig::default())
        .map_err(|e| StartError::Cache(e.to_string()))?;
    // ALLOC: startup only.
    let service = Arc::new(SearchService::new(
        store,
        Arc::new(cache),
        base,
        node_id,
        budget,
    ));
    let router = routes(Router::new(), Arc::clone(&service)).map_err(StartError::Route)?;
    let server =
        oss_rest::Server::start(listener, config, router, budget).map_err(StartError::Server)?;
    Ok((server, service))
}

/// Locks the pushed commits, taking them over if a push panicked holding
/// them: every change to them is one whole step.
pub(crate) fn lock_pushed(
    pushed: &Mutex<PushedCommits>,
) -> std::sync::MutexGuard<'_, PushedCommits> {
    pushed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
