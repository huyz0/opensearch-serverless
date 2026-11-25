//! The document count `max_docs` asks of the index a rollover leaves: the
//! sum of the exact totals of a no-hit match-all search of each shard.

use oss_coordinator::route::resolve::Refusal;
#[cfg(feature = "reader")]
use {
    crate::search::SearchService, oss_blob::BlobStore, oss_reader::dsl::SearchSource,
    std::sync::Arc,
};

/// How many documents an index holds.
pub trait DocCount: Send + Sync {
    /// The documents of `index`, which has `shards` shards.
    ///
    /// # Errors
    ///
    /// The refusal a shard's open or search answers.
    fn count(&self, index: &str, shards: u32) -> Result<u64, Refusal>;
}

#[cfg(feature = "reader")]
/// [`DocCount`] over this node's [`SearchService`]: one query of no hits a
/// shard, so the cost is that of opening the shard once and counting it.
pub struct SearchCount<S: BlobStore + 'static> {
    search: Arc<SearchService<S>>,
}

#[cfg(feature = "reader")]
impl<S: BlobStore + 'static> SearchCount<S> {
    /// Counts through `search`.
    pub fn new(search: Arc<SearchService<S>>) -> Self {
        Self { search }
    }
}

#[cfg(feature = "reader")]
impl<S: BlobStore + 'static> DocCount for SearchCount<S> {
    fn count(&self, index: &str, shards: u32) -> Result<u64, Refusal> {
        let source = SearchSource {
            size: 0,
            track_total_hits: Some(u64::from(i32::MAX.cast_unsigned())),
            ..SearchSource::default()
        };
        let mut total: u64 = 0;
        for shard in 0..shards {
            let found = self
                .search
                .query_shard(index, shard, &source)
                .map_err(|e| Refusal::new(e.status, e.failure.kind, e.failure.reason))?;
            total = total.saturating_add(found.hits.total);
        }
        Ok(total)
    }
}
