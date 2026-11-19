//! A search over all of an index's shards, run by this node's own readers.
//!
//! The coordinator's merge needs each shard's hits and then the page's hits
//! written from the shards that ranked them. Here every shard is a shard of
//! this node's [`SearchService`]; a node that forwards to another is a
//! different [`Shards`].

use std::fmt;
use std::io;

use oss_blob::BlobStore;
use oss_coordinator::reduce::hits::SortSpec;
use oss_coordinator::search::{Params, SearchFailure, Shards, search};
use oss_reader::dsl::{SearchSource, SortKey};
use oss_reader::failure::{Failure, SearchError};
use oss_reader::search::Hits;

use super::{SearchService, ShardQuery};

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
}

impl<S: BlobStore + 'static> Shards for Local<'_, S> {
    type Query = ShardQuery<S>;
    type Error = ShardError;

    fn index(&self, _shard: usize) -> &str {
        self.index
    }

    fn query(&self, shard: usize) -> Result<ShardQuery<S>, ShardError> {
        let shard = u32::try_from(shard).unwrap_or(u32::MAX);
        self.service
            .query_shard(self.index, shard, &self.source)
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
            .map_err(|e| {
                // ALLOC: error path; one box.
                ShardError(Box::new(SearchError::request(
                    500,
                    Failure::plain("exception", e.to_string()),
                )))
            })
    }
}

/// The sort the merge orders by; `None` for the score.
fn sorts(source: &SearchSource) -> Option<Vec<SortSpec>> {
    if source.sort.is_empty() {
        return None;
    }
    Some(
        source
            .sort
            .iter()
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
        let params = Params {
            from: source.from,
            size: source.size,
            sorts: sorts(source),
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
        };
        search(&local, shards, &params)
    }
}
