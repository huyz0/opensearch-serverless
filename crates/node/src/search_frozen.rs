//! A search of a point in time: the commit a view froze, opened like an
//! index and kept in the open-indexes pool under the view's id.

use std::io::Write;
use std::time::Instant;

use oss_blob::BlobStore;
use oss_format::{PointInTime, looks_like_an_id};
use oss_meta::{PitClock, PointInTimeStore};
use oss_reader::dsl::SearchSource;
use oss_reader::failure::{Failure, SearchError, ShardFailure};
use oss_reader::response::write_response_with;
use oss_reader::search::{hits_bytes, search};
use oss_reader::{ReaderError, ShardFiles};
use oss_rest::{JSON, Length, Request, Response, RestError};

use super::{Answer, SearchService, search_error};
use crate::open_indexes::{Found, Lead};
use crate::time_value::{MAX_KEEP_ALIVE_MILLIS, time_millis};

/// Where a search finds its view.
pub(super) struct PitAccess {
    store: PointInTimeStore,
    clock: PitClock,
    buffer_bytes: usize,
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Installs where a search of a point in time reads its view: `store`,
    /// telling the time by `clock`, with one buffer of `buffer_bytes` a
    /// search charged to the request-body pool. False when one is installed
    /// already.
    pub fn set_point_in_time(
        &self,
        store: PointInTimeStore,
        clock: PitClock,
        buffer_bytes: usize,
    ) -> bool {
        self.pits
            .set(PitAccess {
                store,
                clock,
                buffer_bytes,
            })
            .is_ok()
    }

    /// Closes the shards of view `id` this node holds open, so a released
    /// view frees its `open_indexes` charge now rather than when it is the
    /// least recently searched. A search still running on one finishes on it.
    /// Returns how many shards were closed.
    pub fn close_view(&self, id: &str) -> usize {
        // ALLOC: one prefix a release of a view.
        let prefix = format!("pit:{id}#");
        let closed = self.lock().release_prefix(&prefix);
        let count = closed.len();
        // Dropped with the set's lock released.
        drop(closed);
        count
    }

    /// Handles a `_search` that names no index: it names a view.
    pub(super) fn handle_pit(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        source: &SearchSource,
        started: Instant,
    ) -> Result<(), Answer> {
        let Some(pit) = source.pit.as_ref() else {
            return Err(Answer::Rest(RestError::illegal_argument(
                "a search without an index must name a point in time",
            )));
        };
        let Some(access) = self.pits.get() else {
            return Err(Answer::Rest(RestError::illegal_argument(
                "points in time are not supported by this node",
            )));
        };
        let keep_alive = match pit.keep_alive.as_deref() {
            Some(v) => {
                let millis = time_millis("keep_alive", v).map_err(Answer::Rest)?;
                if millis > MAX_KEEP_ALIVE_MILLIS {
                    return Err(Answer::Rest(RestError::illegal_argument(
                        "keep_alive may be at most 1h",
                    )));
                }
                Some(millis)
            }
            None => None,
        };
        if !looks_like_an_id(&pit.id) {
            return Err(missing(&pit.id));
        }
        let view = self.read_view(req, access, &pit.id, keep_alive)?;
        let [frozen] = view.shards.as_slice() else {
            #[cfg(feature = "coordinator")]
            return match self.handle_view(resp, &view, &pit.id, source, started) {
                Ok(()) => Ok(()),
                Err(e) => Err(Answer::Rest(e)),
            };
            #[cfg(not(feature = "coordinator"))]
            return Err(Answer::Rest(RestError::illegal_argument(format!(
                "searching a point in time of [{}] shards needs the coordinator role",
                view.shards.len()
            ))));
        };
        let held = self.open_frozen(&view, frozen.shard, &pit.id)?;
        req.body()
            .reserve(hits_bytes(source, &held.shard))
            .map_err(Answer::Rest)?;
        let analyzer = lucene_analysis::Analyzer::standard(None);
        let mut hits = match search(&held.shard, &view.index, source, &analyzer) {
            Ok(hits) => hits,
            Err(e) => {
                let failure = ShardFailure {
                    // ALLOC: error path; the index name.
                    index: view.index.clone(),
                    shard: frozen.shard,
                    // ALLOC: error path; the node id.
                    node: self.node_id.clone(),
                };
                return Err(search_error(SearchError::on_shard(
                    e.status, e.failure, failure,
                )));
            }
        };
        if let Some(cap) = source.response_size_cap {
            hits.truncate(cap);
        }
        let took = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let io = |e: std::io::Error| Answer::Rest(RestError::from(e));
        let mut body = resp
            .start(200, Some(JSON), Length::Chunked, &[])
            .map_err(Answer::Rest)?;
        let pool = req.body();
        let mut reserve = |n| {
            pool.reserve(n)
                // ALLOC: error path; one box.
                .map_err(|_| std::io::Error::other("the request-body pool is full"))
        };
        write_response_with(
            &mut body,
            &held.shard,
            &view.index,
            &self.node_id,
            &hits,
            took,
            Some(&pit.id),
            &mut reserve,
        )
        .map_err(io)?;
        body.flush().map_err(io)?;
        body.finish().map_err(Answer::Rest)
    }

    /// The live, unexpired view `id`, its deadline moved to now plus
    /// `keep_alive` when given. The buffer is charged before it is made.
    fn read_view(
        &self,
        req: &mut Request<'_>,
        access: &PitAccess,
        id: &str,
        keep_alive: Option<u64>,
    ) -> Result<PointInTime, Answer> {
        req.body()
            .reserve(access.buffer_bytes)
            .map_err(Answer::Rest)?;
        // ALLOC: one buffer a search of a view, buffer_bytes, reserved above.
        let mut buf = vec![0u8; access.buffer_bytes];
        let now = (access.clock)();
        let failed = |e: oss_meta::MetaError| {
            // ALLOC: error path; the failure's words.
            Answer::Rest(RestError::exception(
                500,
                oss_rest::Exception::new("point_in_time_failure", e.to_string()),
            ))
        };
        let view = access
            .store
            .read(id, &mut buf)
            .map_err(failed)?
            .filter(|view| !view.expired_at(now))
            .ok_or_else(|| missing(id))?;
        if let Some(millis) = keep_alive
            && !access
                .store
                .extend(id, now.saturating_add(millis), None, &mut buf)
                .map_err(failed)?
        {
            return Err(missing(id));
        }
        Ok(view)
    }

    /// The open shard `shard` of `view`, opened now if it is not yet.
    pub(super) fn open_frozen(
        &self,
        view: &PointInTime,
        shard: u32,
        id: &str,
    ) -> Result<std::sync::Arc<super::Held<S>>, Answer> {
        // An index name holds no ':', so this key is no index's.
        // ALLOC: one key a search of a view.
        let key = format!("pit:{id}#{shard}");
        loop {
            // The lock is released before waiting.
            let found = self.lock().find(&key);
            match found {
                Found::Open(held) => return Ok(held),
                Found::Wait(opening) => opening.wait(),
                Found::Lead => break,
            }
        }
        let lead = Lead::new(&self.open, &key);
        let (files, meta) =
            ShardFiles::open_frozen(&self.store, &self.base, &view.index, view, shard).map_err(
                |e| match e {
                    ReaderError::NotHeld | ReaderError::NotFrozen(_) => missing(id),
                    e => super::open_shard::open_failure(&view.index, e),
                },
            )?;
        let held = self.open_held(&view.index, shard, files, meta, false, false)?;
        Ok(lead.opened(held))
    }
}

/// `SearchContextMissingException`: the view does not exist, was released
/// or expired.
fn missing(id: &str) -> Answer {
    // ALLOC: error path; one box and one message.
    Answer::Search(Box::new(SearchError::request(
        404,
        Failure::opensearch(
            "search_context_missing_exception",
            format!("No search context found for id [{id}]"),
        ),
    )))
}
