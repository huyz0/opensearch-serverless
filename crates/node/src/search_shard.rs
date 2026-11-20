//! One shard's query for a coordinator in the same process, and the hits it
//! keeps from it.
//!
//! The query runs on the shard (with its overlay when one is ready) and the
//! result stays with the open shard, so the hits a merge keeps are written
//! from it afterwards as the shard's own response writes them.

use std::io;
use std::sync::Arc;

use lucene_analysis::Analyzer;
use oss_blob::BlobStore;
use oss_format::PointInTime;
use oss_reader::OpenShard;
use oss_reader::dsl::SearchSource;
use oss_reader::failure::{Failure, SearchError, ShardFailure};
use oss_reader::response::render_selected_result_hits;
use oss_reader::search::{Hits, hits_bytes, search};

use super::open_shard::into_error;
use super::{Held, SearchService};

/// A shard's answer to a query, and the shard it came from.
pub struct ShardQuery<S: BlobStore + 'static> {
    held: Arc<Held<S>>,
    view: Option<Arc<OpenShard<S>>>,
    index: String,
    /// What the shard found.
    pub hits: Hits,
}

impl<S: BlobStore + 'static> ShardQuery<S> {
    /// Each hit at `positions` of [`Self::hits`] written as the shard's own
    /// response writes it, in the order given; `reserve` grants the buffers
    /// a hit's fields are read into.
    ///
    /// # Errors
    ///
    /// An error when no position is given, when one lies beyond the hits, or
    /// when a hit cannot be fetched or written.
    pub fn render(
        &self,
        node_id: &str,
        positions: &[usize],
        reserve: &mut dyn FnMut(usize) -> io::Result<()>,
    ) -> io::Result<Vec<Vec<u8>>> {
        let shard = self.view.as_deref().unwrap_or(&self.held.shard);
        render_selected_result_hits(shard, &self.index, node_id, &self.hits, positions, reserve)
    }
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Runs `source` on shard `shard` of `index`, opening it first if it is
    /// not open.
    ///
    /// # Errors
    ///
    /// The search error an open or the query answers, naming the shard.
    pub fn query_shard(
        &self,
        index: &str,
        shard: u32,
        source: &SearchSource,
    ) -> Result<ShardQuery<S>, Box<SearchError>> {
        self.query_shard_reserving(index, shard, source, &|_| Ok(()))
    }

    /// As [`Self::query_shard`], first asking `reserve` for the bytes this
    /// shard's hits take; a refusal fails the shard before it searches.
    ///
    /// # Errors
    ///
    /// The search error an open, the reservation or the query answers,
    /// naming the shard.
    pub fn query_shard_reserving(
        &self,
        index: &str,
        shard: u32,
        source: &SearchSource,
        reserve: &dyn Fn(usize) -> std::io::Result<()>,
    ) -> Result<ShardQuery<S>, Box<SearchError>> {
        let held = self.open_shard(index, shard, false).map_err(into_error)?;
        self.query_held(held, index, shard, source, reserve, true)
    }

    /// As [`Self::query_shard_reserving`], over shard `shard` of the view
    /// `id` froze, opened now if it is not yet. A frozen shard has no
    /// overlay: it is the commit the view named, and no later write.
    ///
    /// # Errors
    ///
    /// The search error an open, the reservation or the query answers,
    /// naming the shard.
    pub fn query_frozen_reserving(
        &self,
        view: &PointInTime,
        id: &str,
        shard: u32,
        source: &SearchSource,
        reserve: &dyn Fn(usize) -> std::io::Result<()>,
    ) -> Result<ShardQuery<S>, Box<SearchError>> {
        let held = self.open_frozen(view, shard, id).map_err(into_error)?;
        self.query_held(held, &view.index, shard, source, reserve, false)
    }

    fn query_held(
        &self,
        held: Arc<Held<S>>,
        index: &str,
        shard: u32,
        source: &SearchSource,
        reserve: &dyn Fn(usize) -> std::io::Result<()>,
        overlay: bool,
    ) -> Result<ShardQuery<S>, Box<SearchError>> {
        reserve(hits_bytes(source, &held.shard)).map_err(|e| {
            // ALLOC: error path; one box and the names.
            Box::new(SearchError::on_shard(
                429,
                Failure::plain("circuit_breaking_exception", e.to_string()),
                ShardFailure {
                    index: index.to_owned(),
                    shard,
                    node: self.node_id.clone(),
                },
            ))
        })?;
        let view = if overlay {
            self.overlay_view(&held)
        } else {
            None
        };
        let analyzer = Analyzer::standard(None);
        let hits = {
            let open = view.as_deref().unwrap_or(&held.shard);
            search(open, index, source, &analyzer).map_err(|e| {
                // ALLOC: error path; the index name and the node id.
                Box::new(SearchError::on_shard(
                    e.status,
                    e.failure,
                    ShardFailure {
                        index: index.to_owned(),
                        shard,
                        node: self.node_id.clone(),
                    },
                ))
            })?
        };
        Ok(ShardQuery {
            held,
            view,
            // ALLOC: one name a query.
            index: index.to_owned(),
            hits,
        })
    }
}
