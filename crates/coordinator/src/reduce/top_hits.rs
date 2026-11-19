//! `InternalTopHits.reduce`: one `top_hits` aggregation merged across the
//! shards that answered.
//!
//! Each shard ranked its top `from + size` documents of the aggregation's own
//! sort. The merge orders all of them with [`Merger`], applies `from` once
//! over the whole, and keeps `size`: a hit a shard ranked ahead of its own
//! `from` can land in the page. The page names its hits as a shard and a
//! position; fetching them from their shards is the caller's.

use std::io::Write;

use oss_reader::aggs::metric::top_hits::TopHitsResult;
use oss_reader::response::write_java_float;

use crate::fanout::{self, TaskError};
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

/// Why a merged page could not be rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError<E> {
    /// `shard`'s fetch returned an error.
    Failed {
        /// The shard's index among the merged results.
        shard: usize,
        /// What the fetch returned.
        error: E,
    },
    /// `shard`'s fetch panicked.
    Panicked {
        /// The shard's index among the merged results.
        shard: usize,
    },
    /// `shard` answered a number of hits other than the number asked.
    WrongCount {
        /// The shard's index among the merged results.
        shard: usize,
        /// How many hits were asked.
        asked: usize,
        /// How many came back.
        got: usize,
    },
}

/// Fetches the hits of `merged`'s page from their shards and writes the
/// aggregation's `"hits":{..}` body as one shard writes it.
///
/// `fetch` is called once for each shard the page draws on, at most
/// `concurrency` at a time, with the shard's index among the merged results
/// and the positions wanted (as `merged.page` names them); it returns each
/// hit's JSON object in the order asked. The hits come out in page order.
///
/// # Errors
///
/// [`RenderError`] naming the first shard (in page order) whose fetch failed,
/// panicked, or answered a number of hits other than the number asked.
pub fn render_page<E, F>(
    merged: &TopHitsMerged,
    concurrency: usize,
    fetch: F,
) -> Result<Vec<u8>, RenderError<E>>
where
    E: Send,
    F: Fn(usize, &[usize]) -> Result<Vec<Vec<u8>>, E> + Sync,
{
    // ALLOC: one entry a shard the page draws on, at most the merged shards.
    let mut asks: Vec<(usize, Vec<usize>)> = Vec::new();
    for &(shard, at) in &merged.page {
        match asks.iter_mut().find(|(s, _)| *s == shard) {
            Some((_, positions)) => positions.push(at),
            // ALLOC: one list a shard, at most the page's hits.
            None => asks.push((shard, vec![at])),
        }
    }
    let answers = fanout::run(concurrency, &asks, |(shard, positions)| {
        fetch(*shard, positions)
    });
    // ALLOC: one cursor a shard asked.
    let mut hits: Vec<(usize, std::vec::IntoIter<Vec<u8>>)> = Vec::new();
    for ((shard, positions), answer) in asks.iter().zip(answers) {
        match answer {
            Err(TaskError::Failed(error)) => {
                return Err(RenderError::Failed {
                    shard: *shard,
                    error,
                });
            }
            Err(TaskError::Panicked) => return Err(RenderError::Panicked { shard: *shard }),
            Ok(rendered) if rendered.len() != positions.len() => {
                return Err(RenderError::WrongCount {
                    shard: *shard,
                    asked: positions.len(),
                    got: rendered.len(),
                });
            }
            Ok(rendered) => hits.push((*shard, rendered.into_iter())),
        }
    }
    // ALLOC: the body, bounded by the page's rendered hits.
    let mut out = Vec::new();
    // Writing to a vector cannot fail.
    let _ = write!(
        out,
        "\"hits\":{{\"total\":{{\"value\":{},\"relation\":\"eq\"}},\"max_score\":",
        merged.total
    );
    match merged.max_score {
        Some(score) => {
            let _ = write_java_float(&mut out, score);
        }
        None => out.extend_from_slice(b"null"),
    }
    out.extend_from_slice(b",\"hits\":[");
    for (i, &(shard, _)) in merged.page.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        let hit = hits
            .iter_mut()
            .find(|(s, _)| *s == shard)
            .and_then(|(_, it)| it.next());
        // Every shard answered as many as it was asked, so one is left.
        out.extend_from_slice(&hit.unwrap_or_default());
    }
    out.extend_from_slice(b"]}");
    Ok(out)
}
