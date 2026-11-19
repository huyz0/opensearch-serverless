//! `InternalTopHits.reduce`: one `top_hits` aggregation merged across the
//! shards that answered.
//!
//! Each shard ranked its top `from + size` documents of the aggregation's own
//! sort. The merge orders all of them with [`Merger`], applies `from` once
//! over the whole, and keeps `size`: a hit a shard ranked ahead of its own
//! `from` can land in the page. The page names its hits as a shard and a
//! position; fetching them from their shards is the caller's.

use oss_reader::aggs::metric::top_hits::TopHitsResult;

use crate::reduce::hits::{ACCURATE, MergeError, Merger, ShardAnswer, ShardHit, SortSpec};

/// A `top_hits` merged across shards.
#[derive(Clone, Debug, PartialEq)]
pub struct TopHitsMerged {
    /// How many documents matched in all, exactly.
    pub total: u64,
    /// The best score any shard had; `None` when none had one.
    pub max_score: Option<f32>,
    /// The page's hits in order, each as the shard's index in `shards` and
    /// the hit's position among that shard's `before` then `hits`.
    pub page: Vec<(usize, usize)>,
    /// The most hits the merge held at once.
    pub peak_held: usize,
}

/// Merges `shards`' results of one `top_hits` asking `from` and `size`, in
/// the order of `sorts` (by score when `None`).
///
/// # Errors
///
/// [`MergeError::IncompatibleSortValues`] when two shards' sort values at one
/// key are of different kinds.
pub fn reduce_top_hits(
    shards: &[TopHitsResult],
    from: usize,
    size: usize,
    sorts: Option<Vec<SortSpec>>,
) -> Result<TopHitsMerged, MergeError> {
    let mut merger: Merger<(usize, usize)> = Merger::new(from, size, sorts);
    for (shard, result) in shards.iter().enumerate() {
        let hits = result
            .before
            .iter()
            .chain(&result.hits)
            .enumerate()
            .map(|(at, hit)| ShardHit {
                score: hit.score,
                sort: hit.sort.clone(),
                payload: (shard, at),
            })
            // ALLOC: one entry a hit the shard kept, at most the inner result window.
            .collect();
        merger.absorb(ShardAnswer {
            total: result.total,
            exact: true,
            max_score: result.max_score,
            hits,
            timed_out: false,
            terminated_early: None,
        })?;
    }
    let peak_held = merger.peak_held();
    let merged = merger.finish(Some(ACCURATE))?;
    Ok(TopHitsMerged {
        total: merged.total,
        max_score: merged.max_score,
        // ALLOC: the page, at most size entries.
        page: merged.page.into_iter().map(|h| h.payload).collect(),
        peak_held,
    })
}
