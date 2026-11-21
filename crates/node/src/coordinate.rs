//! A search over all of an index's shards, run by this node's own readers.
//!
//! The coordinator's merge needs each shard's hits and then the page's hits
//! written from the shards that ranked them. Here every shard is a shard of
//! this node's [`SearchService`]; a node that forwards to another is a
//! different [`Shards`].

use std::fmt;
use std::io::{self, Write};
use std::sync::Mutex;
use std::time::Instant;

use oss_blob::BlobStore;
use oss_coordinator::reduce::aggs::AggsError;
use oss_coordinator::reduce::hits::SortSpec;
use oss_coordinator::search::{Params, SearchFailure, Shards, TopHitsWindow, search};
use oss_format::PointInTime;
use oss_mem::{Pool, Reservation};
use oss_reader::aggs::bucket::BUCKET_LIMIT;
use oss_reader::aggs::bucket::terms_collect::too_many_buckets;
use oss_reader::dsl::{SearchSource, SortKey};
use oss_reader::failure::{Failure, SearchError};
use oss_reader::search::Hits;
use oss_rest::{JSON, Length, Response, RestError};

use super::{Answer, SearchService, ShardQuery, send};

/// A search to run once the response's buffers can be reserved: given the
/// `took` it reports and what grants a buffer.
type Run<'a> = dyn Fn(u64, &(dyn Fn(usize) -> io::Result<()> + Sync)) -> Result<Vec<u8>, SearchFailure<ShardError>>
    + 'a;

/// A shard's error, as the text a failure lists.
#[derive(Debug)]
pub struct ShardError(Box<SearchError>);

impl fmt::Display for ShardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut body = Vec::new();
        // Writing to a vector cannot fail.
        let _ = self.0.write_body(&mut body);
        f.write_str(&String::from_utf8_lossy(&body))
    }
}

impl ShardError {
    /// The search error behind it.
    #[must_use]
    pub fn into_inner(self) -> Box<SearchError> {
        self.0
    }
}

/// An index's shards on this node, asked for their top `from + size`.
struct Local<'a, S: BlobStore + 'static> {
    service: &'a SearchService<S>,
    index: &'a str,
    source: SearchSource,
    reserve: &'a (dyn Fn(usize) -> io::Result<()> + Sync),
    /// The view and its id, when the shards are the ones it froze: the
    /// merge's shard `n` is then the view's `n`th shard.
    frozen: Option<(&'a PointInTime, &'a str)>,
}

impl<S: BlobStore + 'static> Shards for Local<'_, S> {
    type Query = ShardQuery<S>;
    type Error = ShardError;

    fn index(&self, _shard: usize) -> &str {
        self.index
    }

    fn query(&self, shard: usize) -> Result<ShardQuery<S>, ShardError> {
        if let Some((view, id)) = self.frozen {
            let number = view.shards.get(shard).map_or(u32::MAX, |s| s.shard);
            return self
                .service
                .query_frozen_reserving(view, id, number, &self.source, self.reserve)
                .map_err(ShardError);
        }
        let shard = u32::try_from(shard).unwrap_or(u32::MAX);
        self.service
            .query_shard_reserving(self.index, shard, &self.source, self.reserve)
            .map_err(ShardError)
    }

    fn hits<'q>(&self, query: &'q ShardQuery<S>) -> &'q Hits {
        &query.hits
    }

    fn render(
        &self,
        _shard: usize,
        query: &ShardQuery<S>,
        positions: &[usize],
    ) -> Result<Vec<Vec<u8>>, ShardError> {
        query
            .render(&self.service.node_id, positions, &mut |n| (self.reserve)(n))
            .map_err(render_error)
    }

    fn top_hits_window(&self, name: &str) -> Option<TopHitsWindow> {
        let spec = self
            .source
            .aggs
            .iter()
            .find(|a| a.name == name)?
            .top_hits
            .as_deref()?;
        Some(TopHitsWindow {
            from: spec.from,
            size: spec.size,
            sorts: sorts(&spec.sort),
        })
    }

    fn render_top_hits(
        &self,
        _shard: usize,
        query: &ShardQuery<S>,
        name: &str,
        positions: &[usize],
    ) -> Result<Vec<Vec<u8>>, ShardError> {
        query
            .render_top_hits(&self.service.node_id, name, positions, &mut |n| {
                (self.reserve)(n)
            })
            .map_err(render_error)
    }
}

fn render_error(e: io::Error) -> ShardError {
    // ALLOC: error path; one box.
    ShardError(Box::new(SearchError::request(
        500,
        Failure::plain("exception", e.to_string()),
    )))
}

/// The sort the merge orders by; `None` for the score.
fn sorts(keys: &[SortKey]) -> Option<Vec<SortSpec>> {
    if keys.is_empty() {
        return None;
    }
    Some(
        keys.iter()
            .map(|key| {
                let ascending = match key {
                    SortKey::Score { ascending } | SortKey::Field { ascending, .. } => *ascending,
                };
                SortSpec {
                    descending: !ascending,
                    missing_first: false,
                }
            })
            // ALLOC: one entry a sort key, at most the parser's bound.
            .collect(),
    )
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Runs `source` over shards `0..shards` of `index` on this node and
    /// writes the merged response, `took_ms` being what it reports.
    /// `reserve` grants the buffers a hit's fields are read into.
    ///
    /// # Errors
    ///
    /// [`SearchFailure`]: every shard failed, the answers do not merge, or a
    /// shard could not write its part of the page.
    pub fn coordinate(
        &self,
        index: &str,
        shards: usize,
        source: &SearchSource,
        took_ms: u64,
        reserve: &(dyn Fn(usize) -> io::Result<()> + Sync),
    ) -> Result<Vec<u8>, SearchFailure<ShardError>> {
        self.coordinate_over(index, None, shards, source, took_ms, reserve)
    }

    /// As [`Self::coordinate`], over the shards of the view `id` froze.
    ///
    /// # Errors
    ///
    /// As [`Self::coordinate`].
    pub fn coordinate_view(
        &self,
        view: &PointInTime,
        id: &str,
        source: &SearchSource,
        took_ms: u64,
        reserve: &(dyn Fn(usize) -> io::Result<()> + Sync),
    ) -> Result<Vec<u8>, SearchFailure<ShardError>> {
        self.coordinate_over(
            &view.index,
            Some((view, id)),
            view.shards.len(),
            source,
            took_ms,
            reserve,
        )
    }

    fn coordinate_over(
        &self,
        index: &str,
        frozen: Option<(&PointInTime, &str)>,
        shards: usize,
        source: &SearchSource,
        took_ms: u64,
        reserve: &(dyn Fn(usize) -> io::Result<()> + Sync),
    ) -> Result<Vec<u8>, SearchFailure<ShardError>> {
        let params = Params {
            from: source.from,
            size: source.size,
            sorts: sorts(&source.sort),
            track_total_hits_up_to: source
                .track_total_hits
                .map(|n| i64::try_from(n).unwrap_or(i64::MAX)),
            show_total: source.track_total_hits.is_some(),
            concurrency: shards.min(8),
            took_ms,
        };
        // Each shard keeps its own top `from + size`; the merge applies `from`.
        let mut per_shard = source.clone();
        per_shard.size = source.from.saturating_add(source.size);
        per_shard.from = 0;
        let local = Local {
            service: self,
            index,
            source: per_shard,
            reserve,
            frozen,
        };
        search(&local, shards, &params)
    }
}

/// What a search that failed answers.
pub(super) fn refusal(failure: SearchFailure<ShardError>) -> Box<SearchError> {
    let (status, failure) = match failure {
        SearchFailure::AllFailed { error } => return error.into_inner(),
        SearchFailure::Merge(e) => (
            400,
            Failure::plain("illegal_argument_exception", e.to_string()),
        ),
        SearchFailure::Aggs(AggsError::TooManyBuckets { made }) => {
            let refused = too_many_buckets(made, BUCKET_LIMIT);
            (refused.status, refused.failure)
        }
        SearchFailure::Aggs(e) => (
            501,
            // ALLOC: error path; one message.
            Failure::plain(
                "status_exception",
                format!("cannot reduce aggregations: {e:?}"),
            ),
        ),
        SearchFailure::Fetch { shard, reason } => (
            500,
            // ALLOC: error path; one message.
            Failure::plain(
                "exception",
                format!("shard [{shard}] could not fetch its hits: {reason}"),
            ),
        ),
    };
    // ALLOC: error path; one box.
    Box::new(SearchError::request(status, failure))
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// How many shards `index` has, opening its shard 0 if it is not open.
    pub(super) fn shard_count(&self, index: &str) -> Result<usize, Answer> {
        let held = self.open_shard(index, 0, false)?;
        Ok(usize::try_from(held.shard.meta.number_of_shards).unwrap_or(usize::MAX))
    }

    /// Answers `source` over the `shards` shards of `index` as one response.
    pub(super) fn handle_sharded(
        &self,
        resp: &mut Response<'_>,
        index: &str,
        shards: usize,
        source: &SearchSource,
        started: Instant,
    ) -> Result<(), RestError> {
        self.handle_sharded_with(resp, index, shards, source, started, &|bytes| bytes)
    }

    /// The merged response of `source` over the shards of `index`, as bytes.
    pub(super) fn coordinate_bytes(
        &self,
        index: &str,
        source: &SearchSource,
        started: Instant,
    ) -> Result<Vec<u8>, super::Answer> {
        let shards = self.shard_count(index)?;
        let held_buffers: Mutex<Vec<Reservation>> = Mutex::new(Vec::new());
        let reserve = |n: usize| {
            let grant = self
                .budget
                .try_reserve(Pool::RequestBodies, n)
                // ALLOC: error path; one box.
                .map_err(|_| io::Error::other("the request-body pool is full"))?;
            held_buffers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                // ALLOC: one entry a render.
                .push(grant);
            Ok(())
        };
        let took = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.coordinate(index, shards, source, took, &reserve)
            .map_err(|failure| super::Answer::Search(refusal(failure)))
    }

    /// [`Self::handle_sharded`], writing what `convert` makes of the merged
    /// response.
    pub(super) fn handle_sharded_with(
        &self,
        resp: &mut Response<'_>,
        index: &str,
        shards: usize,
        source: &SearchSource,
        started: Instant,
        convert: &dyn Fn(Vec<u8>) -> Vec<u8>,
    ) -> Result<(), RestError> {
        self.respond(resp, started, convert, &|took, reserve| {
            self.coordinate(index, shards, source, took, reserve)
        })
    }

    /// Answers `source` over the shards of the view `id` froze, as one
    /// response that names the view first, as a search of one shard of it
    /// does.
    pub(super) fn handle_view(
        &self,
        resp: &mut Response<'_>,
        view: &PointInTime,
        id: &str,
        source: &SearchSource,
        started: Instant,
    ) -> Result<(), RestError> {
        // The id is 22 characters of letters, digits, '-' and '_': it needs
        // no escaping.
        let name = |bytes: Vec<u8>| {
            let mut named =
                Vec::with_capacity(bytes.len().saturating_add(id.len()).saturating_add(12));
            named.extend_from_slice(b"{\"pit_id\":\"");
            named.extend_from_slice(id.as_bytes());
            named.extend_from_slice(b"\",");
            named.extend_from_slice(bytes.get(1..).unwrap_or_default());
            named
        };
        self.respond(resp, started, &name, &|took, reserve| {
            self.coordinate_view(view, id, source, took, reserve)
        })
    }

    fn respond(
        &self,
        resp: &mut Response<'_>,
        started: Instant,
        convert: &dyn Fn(Vec<u8>) -> Vec<u8>,
        run: &Run<'_>,
    ) -> Result<(), RestError> {
        // The buffers each shard's hits take and its renders read into are
        // held until the response is written.
        let held_buffers: Mutex<Vec<Reservation>> = Mutex::new(Vec::new());
        let reserve = |n: usize| {
            let grant = self
                .budget
                .try_reserve(Pool::RequestBodies, n)
                // ALLOC: error path; one box.
                .map_err(|_| io::Error::other("the request-body pool is full"))?;
            held_buffers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                // ALLOC: one entry a render.
                .push(grant);
            Ok(())
        };
        let took = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match run(took, &reserve) {
            Ok(bytes) => {
                let bytes = convert(bytes);
                let mut out = resp.start(200, Some(JSON), Length::Chunked, &[])?;
                out.write_all(&bytes)?;
                out.flush()?;
                out.finish()
            }
            Err(failure) => send(resp, &refusal(failure)),
        }
    }
}
