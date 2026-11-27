//! The owner's answer to a Java coordinator's last-resort ask
//! (`internal:serverless/search/shard`): one shard it writes, searched in its
//! own view, which holds the writes it acknowledged and has not published.
//!
//! The request is read for the part of `SearchSourceBuilder` this node runs
//! and refused by name past it. The node answers only for a shard it holds
//! while its own lease was renewed recently, checked before and after the
//! search from its own state, with no request to the store. This is not write
//! fencing: see ADR 0057.

mod answer_value;
mod convert;
mod local;
mod terms;

pub use local::{Local, run};

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use oss_mem::{MemoryBudget, Pool};
use oss_reader::dsl::SearchSource;
use oss_reader::failure::SearchError;
use oss_reader::response::Row;
use oss_transport::actions::search_shard::{
    AggAnswer, Bounds, Hit, MAX_BYTES, Refusal, Request as Ask, Response,
};
use oss_transport::exception::RemoteException;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::Output;
use oss_transport::version::WireVersion;

use crate::lease_age::LeaseAge;
use crate::writer::WriterRole;

/// What a shard found, before it is written.
pub struct Found {
    /// How many documents matched.
    pub total: u64,
    /// Whether `total` is exact.
    pub exact: bool,
    /// The best score.
    pub max_score: Option<f32>,
    /// Whether `terminate_after` ended the search, when it was asked.
    pub terminated_early: Option<bool>,
    /// The hits in order.
    pub rows: Vec<Row>,
    /// The aggregations' shard-level results, in request order.
    pub aggregations: Vec<oss_reader::aggs::partial::Partial>,
}

/// The shards this node writes, and the search of one.
pub trait Shards: Send + Sync {
    /// The uuid of the index called `index`, as this node knows it.
    fn uuid_of(&self, index: &str) -> Option<String>;
    /// Whether this node holds shard `shard` of the index `uuid` as writer.
    fn holds(&self, uuid: &str, shard: u32) -> bool;
    /// Runs `source` on the shard, in the view that holds the writes this
    /// node acknowledged; `reserve` grants the bytes it reads into.
    ///
    /// # Errors
    ///
    /// The search error of the open or the query.
    fn search(
        &self,
        index: &str,
        shard: u32,
        source: &SearchSource,
        reserve: &dyn Fn(usize) -> io::Result<()>,
    ) -> Result<Found, Box<SearchError>>;
}

/// How the handler is bounded.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// `serverless.search.shard_action.max_lease_age_millis`.
    pub max_lease_age_millis: u64,
    /// `serverless.search.shard_action.max_concurrent`: searches at once.
    pub max_concurrent: usize,
}

/// Answers `internal:serverless/search/shard` for the shards this node writes.
pub struct SearchShardHandler {
    shards: Arc<dyn Shards>,
    lease: Arc<LeaseAge>,
    budget: MemoryBudget,
    limits: Limits,
    running: AtomicUsize,
}

/// A search slot, given back when dropped.
struct Slot<'a>(&'a AtomicUsize);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn refuse(message: &str, status: &str) -> RemoteException {
    RemoteException::status(message, status)
}

fn status_name(status: u16) -> &'static str {
    match status {
        400 => "BAD_REQUEST",
        404 => "NOT_FOUND",
        429 => "TOO_MANY_REQUESTS",
        s if s < 500 => "BAD_REQUEST",
        _ => "SERVICE_UNAVAILABLE",
    }
}

fn from_refusal(refusal: &Refusal) -> RemoteException {
    // ALLOC: error path; one message.
    let (message, status) = match refusal {
        Refusal::Wire(e) => (
            format!("a search request that is not readable: {e}"),
            "BAD_REQUEST",
        ),
        Refusal::Unsupported(what) => (
            format!("a forwarded search with [{what}] is not supported by this node"),
            "BAD_REQUEST",
        ),
        Refusal::UnsupportedQuery(name) => (
            format!("a forwarded search with a [{name}] query is not supported by this node"),
            "BAD_REQUEST",
        ),
        Refusal::TooLarge(what) => (
            format!("a forwarded search past the limit of [{what}]"),
            "BAD_REQUEST",
        ),
    };
    refuse(&message, status)
}

impl SearchShardHandler {
    /// A handler over `shards`, answering only while `lease` is recent.
    pub fn new(
        shards: Arc<dyn Shards>,
        lease: Arc<LeaseAge>,
        budget: MemoryBudget,
        limits: Limits,
    ) -> Self {
        Self {
            shards,
            lease,
            budget,
            limits,
            running: AtomicUsize::new(0),
        }
    }

    /// Takes a slot, or `None` when `max_concurrent` searches run.
    fn slot(&self) -> Option<Slot<'_>> {
        let before = self.running.fetch_add(1, Ordering::AcqRel);
        if before >= self.limits.max_concurrent {
            self.running.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Slot(&self.running))
    }

    /// Whether this node may answer for the shard now: it holds it, under the
    /// uuid asked, and has renewed its lease recently.
    fn owns(&self, uuid: &str, shard: u32) -> Result<(), RemoteException> {
        if !self.shards.holds(uuid, shard) {
            return Err(refuse(
                "this node does not hold the shard",
                "SERVICE_UNAVAILABLE",
            ));
        }
        match self.lease.millis() {
            Some(age) if age <= self.limits.max_lease_age_millis => Ok(()),
            _ => Err(refuse(
                "this node has not renewed its lease recently",
                "SERVICE_UNAVAILABLE",
            )),
        }
    }
}

impl Handler for SearchShardHandler {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        if request.version < WireVersion::SHELL {
            return Err(refuse(
                "a forwarded search needs a peer of version 3.10.0 or later",
                "BAD_REQUEST",
            ));
        }
        let ask = Ask::read(request.message, &Bounds::DEFAULT).map_err(|e| from_refusal(&e))?;
        let shard =
            u32::try_from(ask.shard).map_err(|_| refuse("a negative shard", "BAD_REQUEST"))?;
        let uuid = self
            .shards
            .uuid_of(&ask.index)
            .ok_or_else(|| refuse("no such index on this node", "SERVICE_UNAVAILABLE"))?;
        if ask.index_uuid.as_ref().is_some_and(|asked| *asked != uuid) {
            return Err(refuse(
                "the index was recreated under another uuid",
                "SERVICE_UNAVAILABLE",
            ));
        }
        self.owns(&uuid, shard)?;
        let _slot = self.slot().ok_or_else(|| {
            RemoteException::rejected("this node is answering as many forwarded searches as it may")
        })?;
        let mut source = convert::search_source(&ask.source).map_err(|e| {
            refuse(
                &format!("a search source that cannot run: {e:?}"),
                "BAD_REQUEST",
            )
        })?;
        // `now` in date math is the coordinator's instant, not this node's.
        source.now_millis = Some(ask.now_millis);
        // The answer's bytes are reserved before they are read.
        let reservation = Mutex::new(
            self.budget
                .try_reserve(Pool::QueryExecution, 0)
                .map_err(|_| RemoteException::rejected("the query pool is full"))?,
        );
        let grow = |bytes: usize| {
            let mut held = reservation.lock().unwrap_or_else(PoisonError::into_inner);
            if held.bytes().saturating_add(bytes) > MAX_BYTES {
                return Err(io::Error::other("the answer would pass its limit"));
            }
            held.try_grow(bytes)
                .map_err(|_| io::Error::other("the query pool is full"))
        };
        let found = self
            .shards
            .search(&ask.index, shard, &source, &grow)
            .map_err(|e| refuse(&e.failure.reason, status_name(e.status)))?;
        // The lease is checked again: a search that outlasted it answers
        // nothing.
        self.owns(&uuid, shard)?;
        write_answer(&found, &ask.source.aggregations, out)
    }
}

fn id_of(row: &Row) -> Result<String, RemoteException> {
    serde_json::from_slice::<String>(&row.id_json)
        .map_err(|_| refuse("a stored id that is not text", "INTERNAL_SERVER_ERROR"))
}

/// The shard's results as the wire writes them: the single-value metrics over
/// a number or a date, which a Java coordinator reduces.
fn answers<'a>(
    parts: &'a [oss_reader::aggs::partial::Partial],
    asked: &[oss_transport::actions::search_shard::AggRequest],
) -> Result<Vec<AggAnswer<'a>>, RemoteException> {
    use oss_reader::aggs::partial::Format;
    // ALLOC: one answer an aggregation, bounded by the request's.
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        let date = p.format == Format::Date;
        let value = answer_value::value(&p.name, &p.value, asked)?;
        out.push(AggAnswer {
            name: &p.name,
            value,
            date,
        });
    }
    Ok(out)
}

fn write_answer(
    found: &Found,
    asked: &[oss_transport::actions::search_shard::AggRequest],
    out: &mut Output<'_>,
) -> Result<(), RemoteException> {
    let ids = found
        .rows
        .iter()
        .map(id_of)
        .collect::<Result<Vec<_>, _>>()?;
    // ALLOC: one hit a row, at most the request's window.
    let hits: Vec<Hit<'_>> = found
        .rows
        .iter()
        .zip(&ids)
        .map(|(row, id)| Hit {
            id,
            score: row.score.unwrap_or(f32::NAN),
            version: row.version.unwrap_or(-1),
            seq_no: row.seq_no.unwrap_or(-2),
            primary_term: row.primary_term.unwrap_or(0),
            source: &row.source,
        })
        .collect();
    let aggregations = answers(&found.aggregations, asked)?;
    Response {
        total: i64::try_from(found.total).unwrap_or(i64::MAX),
        total_is_lower_bound: !found.exact,
        max_score: found.max_score.unwrap_or(f32::NAN),
        timed_out: false,
        terminated_early: found.terminated_early,
    }
    .write(&hits, &aggregations, out)
    .map_err(|_| {
        refuse(
            "the answer is past the limit of a forwarded search",
            "SERVICE_UNAVAILABLE",
        )
    })
}

/// A handler over the node's own writer role and search service.
pub fn handler<S: oss_blob::BlobStore + 'static>(
    role: Arc<WriterRole>,
    search: Arc<crate::search::SearchService<S>>,
    lease: Arc<LeaseAge>,
    budget: MemoryBudget,
    limits: Limits,
) -> SearchShardHandler {
    SearchShardHandler::new(Arc::new(Local::new(role, search)), lease, budget, limits)
}

#[cfg(test)]
mod convert_tests;
#[cfg(test)]
mod tests;
