//! Selected hits of a `top_hits` result, each written as the response writes
//! it, for a coordinator that merged several shards' results and now fetches
//! the hits it kept.

use std::io;

use oss_blob::BlobStore;

use super::{Fetch, HitView, Stored, prefetch, write_hit_list};
use crate::OpenShard;
use crate::aggs::metric::top_hits::TopHitsResult;

/// Each hit at `positions` of `result` (counted over its `before` then its
/// `hits`) written as one JSON object, in the order given. One prefetch
/// announces every selected hit's stored fields, and `reserve` grants the
/// buffers holding a hit's `_id`, `_source` and scratch.
///
/// # Errors
///
/// An error when no position is given, when one lies beyond the result, or
/// when a hit cannot be fetched or written.
pub fn render_selected_hits<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    index: &str,
    node_id: &str,
    result: &TopHitsResult,
    positions: &[usize],
    reserve: &mut dyn FnMut(usize) -> io::Result<()>,
) -> io::Result<Vec<Vec<u8>>> {
    if positions.is_empty() {
        return Err(io::Error::other("no hit selected"));
    }
    let ahead = result.before.len();
    let explained = !result.explanations.is_empty() || !result.before_explanations.is_empty();
    // ALLOC: one hit and explanation a position, at most the inner result window.
    let mut hits = Vec::with_capacity(positions.len());
    // ALLOC: one explanation a position when explained.
    let mut explanations = Vec::new();
    for &at in positions {
        let (hit, explanation) = match at.checked_sub(ahead) {
            None => (result.before.get(at), result.before_explanations.get(at)),
            Some(i) => (result.hits.get(i), result.explanations.get(i)),
        };
        hits.push(
            hit.ok_or_else(|| io::Error::other("a selected hit beyond the result"))?
                .clone(),
        );
        if explained {
            explanations.push(
                explanation
                    .ok_or_else(|| io::Error::other("a selected hit without its explanation"))?
                    .clone(),
            );
        }
    }
    prefetch(shard, &[&hits])?;
    let mut fetch = Fetch {
        stored: Stored::new(reserve),
        scratch: 0,
    };
    // ALLOC: one buffer a position, each the hit's own bytes.
    let mut rendered = Vec::with_capacity(hits.len());
    for (i, hit) in hits.iter().enumerate() {
        let view = HitView {
            hits: std::slice::from_ref(hit),
            source: &result.source,
            fields: &result.fields,
            explanations: explanations.get(i..=i).unwrap_or_default(),
            highlight: result.highlight.as_ref(),
        };
        // ALLOC: the hit's JSON, bounded by its source.
        let mut out = Vec::new();
        write_hit_list(&mut out, shard, index, node_id, &view, &mut fetch)?;
        rendered.push(out);
    }
    Ok(rendered)
}
