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
//! commit only once it is told of one.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::PageCache;
use oss_blob::{BlobError, BlobStore, PooledStore};
use oss_reader::dsl::{DslError, MAX_RECORDED_BYTES, Recorder, SearchSource, source};
use oss_reader::failure::{Failure, Meta, SearchError, ShardFailure};
use oss_reader::response::write_response;
use oss_reader::search::{hits_bytes, search};
use oss_reader::{OpenShard, ReaderError, open_shard};
use oss_rest::{JSON, Length, Method, Request, Response, RestError, RouteError, Router};

/// The most indexes a node keeps open; opening one more closes another.
pub const MAX_OPEN_INDEXES: usize = 1024;

/// The query parameters `_search` takes.
pub const SEARCH_PARAMS: &[&str] = &["size", "from"];

/// Serves searches of the indexes under one deployment base.
pub struct SearchService<S: BlobStore + 'static> {
    store: Arc<PooledStore<S>>,
    cache: Arc<PageCache>,
    base: String,
    node_id: String,
    max_open: usize,
    open: Mutex<HashMap<String, Arc<OpenShard<S>>>>,
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// A service reading `store` under `base`, through `cache`, reporting
    /// shard failures as node `node_id`.
    pub fn new(
        store: Arc<PooledStore<S>>,
        cache: Arc<PageCache>,
        base: &str,
        node_id: &str,
    ) -> Self {
        Self {
            store,
            cache,
            // ALLOC: startup only.
            base: base.to_owned(),
            // ALLOC: startup only.
            node_id: node_id.to_owned(),
            max_open: MAX_OPEN_INDEXES,
            // ALLOC: one entry an open index, at most MAX_OPEN_INDEXES.
            open: Mutex::new(HashMap::new()),
        }
    }

    /// Keeps at most `max` indexes open (at least one) instead of
    /// [`MAX_OPEN_INDEXES`].
    pub fn with_max_open(mut self, max: usize) -> Self {
        self.max_open = max.max(1);
        self
    }

    /// How many indexes are open.
    pub fn open_indexes(&self) -> usize {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// The open shard of `index`, opened now if it is not yet.
    fn shard(&self, index: &str) -> Result<Arc<OpenShard<S>>, Box<SearchError>> {
        if let Some(shard) = self
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(index)
        {
            return Ok(Arc::clone(shard));
        }
        let shard = match open_shard(
            Arc::clone(&self.store),
            Arc::clone(&self.cache),
            &self.base,
            index,
            0,
        ) {
            Ok(shard) => shard,
            Err(ReaderError::Blob(BlobError::NotFound) | ReaderError::Deleted) => {
                return Err(Box::new(index_not_found(index)));
            }
            Err(e) => {
                // ALLOC: error path; one message.
                let failure = Failure::opensearch("exception", e.to_string());
                // ALLOC: error path; one box.
                return Err(Box::new(SearchError::request(500, failure)));
            }
        };
        if shard.meta.number_of_shards != 1 {
            // ALLOC: error path; one box.
            return Err(Box::new(SearchError::request(
                400,
                Failure::plain(
                    "illegal_argument_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "searching an index of [{}] shards is not supported by this node yet",
                        shard.meta.number_of_shards
                    ),
                ),
            )));
        }
        // ALLOC: one open shard an index, at most MAX_OPEN_INDEXES.
        let shard = Arc::new(shard);
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        // A search that opened the same index meanwhile wins; this one's
        // copy is dropped and nothing is evicted for it.
        if let Some(first) = open.get(index) {
            return Ok(Arc::clone(first));
        }
        if open.len() >= self.max_open {
            let evicted = open.keys().next().cloned();
            if let Some(key) = evicted {
                open.remove(&key);
            }
        }
        // ALLOC: the index name, once an open index.
        open.insert(index.to_owned(), Arc::clone(&shard));
        Ok(shard)
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
        let shard = match self.shard(&index) {
            Ok(shard) => shard,
            Err(e) => return send(resp, &e),
        };
        req.body().reserve(hits_bytes(&source, &shard))?;
        let analyzer = Analyzer::standard(None);
        let hits = match search(&shard, &index, &source, &analyzer) {
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
        write_response(&mut body, &shard, &index, &hits, took, &mut reserve)?;
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
    Answer::Search(Box::new(SearchError::request(400, e.into_failure())))
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

/// Why a reader node did not start.
#[derive(Debug)]
pub enum StartError {
    /// The page cache could not be set up.
    Cache(String),
    /// A route could not be registered.
    Route(RouteError),
    /// The HTTP server did not start.
    Server(oss_rest::StartError),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Cache(e) => write!(f, "page cache: {e}"),
            StartError::Route(e) => write!(f, "route: {e:?}"),
            StartError::Server(e) => write!(f, "server: {e:?}"),
        }
    }
}

impl std::error::Error for StartError {}

/// Starts a reader node: `_search` over the indexes under `base` in
/// `store`, served on `listener`, every server buffer charged to `budget`.
pub fn start_reader<S: BlobStore + 'static>(
    listener: std::net::TcpListener,
    config: oss_rest::ServerConfig,
    store: Arc<PooledStore<S>>,
    base: &str,
    node_id: &str,
    budget: &oss_mem::MemoryBudget,
) -> Result<(oss_rest::Server, Arc<SearchService<S>>), StartError> {
    let cache = PageCache::new(lucene_store::page_cache::PageCacheConfig::default())
        .map_err(|e| StartError::Cache(e.to_string()))?;
    // ALLOC: startup only.
    let service = Arc::new(SearchService::new(store, Arc::new(cache), base, node_id));
    let router = routes(Router::new(), Arc::clone(&service)).map_err(StartError::Route)?;
    let server =
        oss_rest::Server::start(listener, config, router, budget).map_err(StartError::Server)?;
    Ok((server, service))
}
