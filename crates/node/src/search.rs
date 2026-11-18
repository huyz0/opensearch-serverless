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

use crate::open_indexes::{Found, Lead, OpenSet, lock};
use lucene_analysis::Analyzer;
use lucene_store::page_cache::PageCache;
use oss_blob::{BlobError, BlobStore, PooledStore};
use oss_mem::{MemoryBudget, Pool, Reservation};
use oss_reader::commit_push::{NewestSource, PushedCommits, ShardKey, Stage};
use oss_reader::dsl::{DslError, MAX_RECORDED_BYTES, Recorder, SearchSource, source};
use oss_reader::failure::{Failure, Meta, SearchError, ShardFailure};
use oss_reader::open_cost::open_bytes;
use oss_reader::overlay::OverlaySource;
use oss_reader::response::write_response;
use oss_reader::search::{hits_bytes, search};
use oss_reader::{OpenShard, ReaderError, ShardFiles, open_located};
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
    charge: Reservation,
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
        }
    }

    /// Installs where a search finds the overlay of the shard it serves.
    /// False when one is installed already.
    pub fn set_overlay_source(&self, source: Arc<dyn OverlaySource>) -> bool {
        self.overlay.set(source).is_ok()
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

    /// Reserves `bytes` from `open_indexes`, closing the least recently
    /// searched indexes no search holds until it fits; refused when none is
    /// left to close.
    fn charge(&self, bytes: usize) -> Result<Reservation, oss_mem::Exhausted> {
        loop {
            match self.budget.try_reserve(Pool::OpenIndexes, bytes) {
                Ok(charge) => return Ok(charge),
                Err(e) => {
                    let closed = {
                        let mut set = self.lock();
                        // Closing what cannot make room would only cost
                        // those indexes a cold open; an index larger than
                        // the whole pool never fits.
                        if !closing_makes_room(&e, set.idle_bytes(|held| held.charge.bytes())) {
                            return Err(e);
                        }
                        set.close_oldest()
                    };
                    // A closed index is held by no search: its engine state
                    // and charge go here, with the set's lock released.
                    if closed.is_none() {
                        return Err(e);
                    }
                }
            }
        }
    }

    /// The open shard of `index`, opened now if it is not yet.
    fn shard(&self, index: &str) -> Result<Arc<Held<S>>, Answer> {
        loop {
            // The lock is released before waiting.
            let found = self.lock().find(index);
            match found {
                Found::Open(held) => return Ok(held),
                Found::Wait(opening) => opening.wait(),
                Found::Lead => break,
            }
        }
        let lead = Lead::new(&self.open, index);
        let (files, meta) = match self.find_commit(index) {
            Ok(found) => found,
            Err(e) => return Err(open_failure(index, e)),
        };
        if meta.number_of_shards != 1 {
            // ALLOC: error path; one box.
            return Err(Answer::Search(Box::new(SearchError::request(
                400,
                Failure::plain(
                    "illegal_argument_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "searching an index of [{}] shards is not supported by this node yet",
                        meta.number_of_shards
                    ),
                ),
            ))));
        }
        let stage = files.stage().clone();
        let bytes = open_bytes(&files, &meta, &self.cache);
        let charge = self.charge(bytes).map_err(|e| {
            Answer::Rest(RestError::breaker(
                &e,
                "open_indexes",
                "<open_index>",
                never_fits(&e),
            ))
        })?;
        let shard = open_located(
            Arc::clone(&self.store),
            Arc::clone(&self.cache),
            &self.base,
            index,
            0,
            files,
            meta,
        )
        .map_err(|e| open_failure(index, e))?;
        // ALLOC: one open shard an index; the pool bounds them.
        Ok(lead.opened(Arc::new(Held {
            shard,
            view: Mutex::new(None),
            stage,
            charge,
        })))
    }

    /// The latest commit of shard 0 of `index`: the one its writer pushed
    /// since this node's reader let go of an older one, which costs the
    /// descriptor's one GET; else the register's, two.
    fn find_commit(&self, index: &str) -> Result<(ShardFiles, oss_reader::IndexMeta), ReaderError> {
        let key = ShardKey::new(index, 0);
        let taken = lock_pushed(&self.pushed).take(&key, self.now_millis());
        if let Some(taken) = taken
            && let Ok((files, meta)) =
                ShardFiles::open_pushed_with_meta(&self.store, &self.base, index, 0, &taken.value)
            && meta.uuid == taken.uuid
        {
            return Ok((files, meta));
        }
        // No push, a push that is no commit, or one for an index since
        // recreated: the register decides, or the newer commit its writer
        // says it staged after it.
        match self.newest.get() {
            Some(source) => ShardFiles::open_asking_with_meta(
                &self.store,
                &self.base,
                index,
                0,
                source.as_ref(),
            ),
            None => ShardFiles::open_with_meta(&self.store, &self.base, index, 0),
        }
    }

    /// Milliseconds since this service started: what a pushed commit's keep
    /// time counts.
    pub(crate) fn now_millis(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Handles one `_search`.
    pub fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let started = Instant::now();
        let Some(index) = req.path_param("index") else {
            return Err(RestError::illegal_argument("no index"));
        };
        let source = match read_source(req) {
            Ok(source) => source,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        let held = match self.shard(&index) {
            Ok(held) => held,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        let view = self.overlay_view(&held, &held.shard.meta.uuid);
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

/// The body, streamed through the recorder, and the URL parameters.
fn read_source(req: &mut Request<'_>) -> Result<SearchSource, Answer> {
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
    let mut source = SearchSource::parse(&rec).map_err(bad_request)?;
    let from = req.param("from");
    let size = req.param("size");
    source
        .apply_params(from.as_deref(), size.as_deref())
        .map_err(bad_request)?;
    Ok(source)
}

/// Whether closing indexes that free `idle` bytes would let the refused
/// reservation `e` fit.
fn closing_makes_room(e: &oss_mem::Exhausted, idle: usize) -> bool {
    !never_fits(e) && idle >= e.in_use.saturating_add(e.requested).saturating_sub(e.limit)
}

/// Whether `e` asked for more than the whole pool: no retry admits it.
fn never_fits(e: &oss_mem::Exhausted) -> bool {
    e.requested > e.limit
}

/// What a failed locate or open answers.
fn open_failure(index: &str, e: ReaderError) -> Answer {
    match e {
        ReaderError::Blob(BlobError::NotFound) | ReaderError::Deleted => {
            // ALLOC: error path; one box.
            Answer::Search(Box::new(index_not_found(index)))
        }
        e => {
            // ALLOC: error path; one message.
            let failure = Failure::opensearch("exception", e.to_string());
            // ALLOC: error path; one box.
            Answer::Search(Box::new(SearchError::request(500, failure)))
        }
    }
}

/// `IndexNotFoundException` for an index no descriptor names.
fn index_not_found(index: &str) -> SearchError {
    SearchError::request(
        404,
        Failure::opensearch(
            "index_not_found_exception",
            // ALLOC: error path; one message.
            format!("no such index [{index}]"),
        )
        // ALLOC: error path; the index name, three times.
        .with_meta("index", Meta::Str(index.to_owned()))
        .with_meta("resource.id", Meta::Str(index.to_owned()))
        .with_meta("resource.type", Meta::Str("index_or_alias".to_owned()))
        .with_meta("index_uuid", Meta::Str("_na_".to_owned())),
    )
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
    router
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
}

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
