//! `POST /{index}/_delete_by_query` over the shards this node writes.
//!
//! The walk is [`oss_coordinator::docs::by_query`]; this module supplies its
//! two halves: the changes, each a delete conditional on the sequence number
//! and primary term the search saw, run through the `_bulk` machinery, and
//! the search, behind [`ShardMatches`], which `SearchMatches` answers from the
//! node's own reader.

mod change;
mod params;
mod response;
#[cfg(feature = "reader")]
mod search;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oss_coordinator::docs::by_query::{Hit, Source, Walk, walk};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};
use oss_writer::bulk::{BulkBudget, Grant, IdGenerator, Target};

pub use change::DeleteChange;
use params::Params;
#[cfg(feature = "reader")]
pub use search::SearchMatches;

use crate::writer::{RoleTarget, WriterRole};

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
}

impl Source for QuerySource<'_> {
    type Error = RestError;
    type Permit<'a>
        = Grant
    where
        Self: 'a;

    fn newest(&self, shard: u32) -> Result<Option<i64>, RestError> {
        self.matches.newest(self.index, shard, self.query)
    }

    fn page(
        &self,
        shard: u32,
        up_to: i64,
        after: Option<i64>,
        size: usize,
    ) -> Result<Vec<Hit>, RestError> {
        self.matches
            .page(self.index, shard, self.query, up_to, after, size)
    }

    fn admit(&self, bytes: u64) -> Result<Grant, RestError> {
        self.budget
            .acquire(usize::try_from(bytes).unwrap_or(usize::MAX), ADMIT_WAIT)
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Serves delete-by-query over a [`WriterRole`].
pub struct ByQueryService {
    role: Arc<WriterRole>,
    matches: Arc<dyn ShardMatches>,
    budget: BulkBudget,
    address: [u8; 6],
}

impl ByQueryService {
    /// A service deleting from `role`'s shards the documents `matches`
    /// finds, each page admitted to `budget`.
    pub fn new(
        role: Arc<WriterRole>,
        matches: Arc<dyn ShardMatches>,
        budget: BulkBudget,
        address: [u8; 6],
    ) -> Self {
        Self {
            role,
            matches,
            budget,
            address,
        }
    }

    /// `POST /{index}/_delete_by_query`.
    pub fn delete(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let started = Instant::now();
        let params = Params::read(req)?;
        let index = req
            .path_param("index")
            .ok_or_else(|| RestError::illegal_argument("an index is required"))?
            .into_owned();
        let info = RoleTarget::new(&self.role)
            .index(&index)
            .ok_or_else(|| missing(&index))?;
        let shards: Vec<u32> = (0..info.shards).collect();
        let source = QuerySource {
            matches: &*self.matches,
            budget: &self.budget,
            index: &index,
            query: &params.query,
        };
        let mut ids = IdGenerator::new(0, self.address);
        let mut change = DeleteChange::new(&self.role, &info, &mut ids, now_millis());
        let outcome: Walk = walk(
            &source,
            &shards,
            params.max_docs,
            params.refresh,
            params.abort_on_conflict,
            &mut change,
        )?;
        let took = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (status, body) = response::render(&outcome, &index, took);
        resp.send(status, JSON, body.as_bytes())
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

/// The delete-by-query route added to `router`.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<ByQueryService>) -> Result<Router, RouteError> {
    router.route(
        Method::Post,
        "/{index}/_delete_by_query",
        params::PARAMS,
        move |req: &mut Request<'_>, resp: &mut Response<'_>| service.delete(req, resp),
    )
}
