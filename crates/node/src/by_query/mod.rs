//! `POST /{index}/_delete_by_query` over the shards this node writes.
//!
//! The walk is [`oss_coordinator::docs::by_query`]; this module supplies its
//! two halves: the changes, each a delete conditional on the sequence number
//! and primary term the search saw, run through the `_bulk` machinery, and
//! the search, behind [`ShardMatches`], which `SearchMatches` answers from the
//! node's own reader.

mod change;
mod params;
mod plan;
mod reindex;
mod response;
#[cfg(feature = "reader")]
mod search;
mod tasks;
mod tasks_route;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oss_coordinator::docs::by_query::{Hit, Source};
use oss_rest::{Method, Request, Response, RestError, RouteError, Router};
use oss_writer::bulk::{BulkBudget, Grant};

pub use change::DeleteChange;
#[cfg(feature = "reader")]
pub use search::SearchMatches;
pub use tasks::Tasks;

use crate::writer::WriterRole;

/// How long a page waits for the write budget.
const ADMIT_WAIT: Duration = Duration::from_secs(30);

/// The search half of a by-query walk: which documents of a shard match a
/// query, in sequence-number order.
pub trait ShardMatches: Send + Sync {
    /// The newest sequence number matching `query` on `shard` of `index`.
    ///
    /// # Errors
    ///
    /// The search's refusal.
    fn newest(&self, index: &str, shard: u32, query: &str) -> Result<Option<i64>, RestError>;

    /// At most `size` matches of `query` on `shard` of `index` with a
    /// sequence number up to `up_to`, after `after` when given, ascending.
    ///
    /// # Errors
    ///
    /// The search's refusal.
    fn page(
        &self,
        index: &str,
        shard: u32,
        query: &str,
        up_to: i64,
        after: Option<i64>,
        size: usize,
    ) -> Result<Vec<Hit>, RestError>;
}

/// The walk's [`Source`]: one query over one index.
struct QuerySource<'a> {
    matches: &'a dyn ShardMatches,
    budget: &'a BulkBudget,
    index: &'a str,
    query: &'a str,
    /// Set by a cancel: the walk then finds nothing more and ends.
    cancel: &'a AtomicBool,
}

impl Source for QuerySource<'_> {
    type Error = RestError;
    type Permit<'a>
        = Grant
    where
        Self: 'a;

    fn newest(&self, shard: u32) -> Result<Option<i64>, RestError> {
        if self.cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        self.matches.newest(self.index, shard, self.query)
    }

    fn page(
        &self,
        shard: u32,
        up_to: i64,
        after: Option<i64>,
        size: usize,
    ) -> Result<Vec<Hit>, RestError> {
        if self.cancel.load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        self.matches
            .page(self.index, shard, self.query, up_to, after, size)
    }

    fn admit(&self, bytes: u64) -> Result<Grant, RestError> {
        self.budget
            .acquire(usize::try_from(bytes).unwrap_or(usize::MAX), ADMIT_WAIT)
    }
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Serves delete-by-query, update-by-query and reindex over a [`WriterRole`].
pub struct ByQueryService {
    role: Arc<WriterRole>,
    matches: Arc<dyn ShardMatches>,
    budget: BulkBudget,
    address: [u8; 6],
    tasks: Arc<Tasks>,
}

impl ByQueryService {
    /// A service deleting from `role`'s shards the documents `matches`
    /// finds, each page admitted to `budget`; background requests are
    /// tracked in `tasks`.
    pub fn new(
        role: Arc<WriterRole>,
        matches: Arc<dyn ShardMatches>,
        budget: BulkBudget,
        address: [u8; 6],
        tasks: Arc<Tasks>,
    ) -> Self {
        Self {
            role,
            matches,
            budget,
            address,
            tasks,
        }
    }
}

fn missing(index: &str) -> RestError {
    RestError::exception(
        404,
        oss_rest::Exception::new(
            "index_not_found_exception",
            // ALLOC: error path; one message.
            format!("no such index [{index}]"),
        )
        .with_meta("index", oss_rest::Meta::Str(index.to_owned())),
    )
}

/// The by-query, reindex and task routes added to `router`.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<ByQueryService>) -> Result<Router, RouteError> {
    let updates = Arc::clone(&service);
    let reindexes = Arc::clone(&service);
    let router = router
        .route(
            Method::Post,
            "/{index}/_delete_by_query",
            params::PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                let plan = service.plan_delete(req)?;
                service.serve(plan, resp)
            },
        )?
        .route(Method::Post, "/{index}/_update_by_query", params::PARAMS, {
            let service = Arc::clone(&updates);
            move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                let plan = service.plan_update(req)?;
                service.serve(plan, resp)
            }
        })?
        .route(Method::Post, "/_reindex", params::PARAMS, {
            let service = Arc::clone(&reindexes);
            move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                let plan = service.plan_reindex(req)?;
                service.serve(plan, resp)
            }
        })?;
    tasks_route::routes(router, updates)
}
