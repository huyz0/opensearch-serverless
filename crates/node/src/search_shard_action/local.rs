//! The shards this node writes, searched through its own search service.

use std::io;
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_reader::dsl::SearchSource;
use oss_reader::failure::{Failure, SearchError};

use super::{Found, Shards};
use crate::search::SearchService;
use crate::writer::WriterRole;

/// The node's writer role and the search service that reads its shards.
pub struct Local<S: BlobStore + 'static> {
    role: Arc<WriterRole>,
    search: Arc<SearchService<S>>,
}

impl<S: BlobStore + 'static> Local<S> {
    /// Shards held by `role`, searched through `search`.
    pub fn new(role: Arc<WriterRole>, search: Arc<SearchService<S>>) -> Self {
        Self { role, search }
    }
}

impl<S: BlobStore + 'static> Shards for Local<S> {
    fn uuid_of(&self, index: &str) -> Option<String> {
        self.role.index_info(index).map(|info| info.uuid)
    }

    fn holds(&self, uuid: &str, shard: u32) -> bool {
        self.role.held(uuid, shard)
    }

    fn search(
        &self,
        index: &str,
        shard: u32,
        source: &SearchSource,
        reserve: &dyn Fn(usize) -> io::Result<()>,
    ) -> Result<Found, Box<SearchError>> {
        run(&self.search, index, shard, source, reserve)
    }
}

/// Runs `source` on shard `shard` of `index` in the view that holds the
/// writes this node acknowledged, and reads the hits' fields.
///
/// # Errors
///
/// The search error of the open, the query or the fetch.
pub fn run<S: BlobStore + 'static>(
    search: &SearchService<S>,
    index: &str,
    shard: u32,
    source: &SearchSource,
    reserve: &dyn Fn(usize) -> io::Result<()>,
) -> Result<Found, Box<SearchError>> {
    let query = search.query_shard_reserving(index, shard, source, reserve)?;
    let rows = query.rows(&mut |n| reserve(n)).map_err(|e| {
        // ALLOC: error path; one box.
        Box::new(SearchError::request(
            503,
            Failure::plain("search_phase_execution_exception", e.to_string()),
        ))
    })?;
    Ok(Found {
        total: query.hits.total,
        exact: query.hits.exact,
        max_score: query.hits.max_score,
        terminated_early: query.hits.terminated_early,
        rows,
    })
}
