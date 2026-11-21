//! Selected hits of a `top_hits` result, each written as the response writes
//! it, for a coordinator that merged several shards' results and now fetches
//! the hits it kept.

use std::io;

use oss_blob::BlobStore;

use lucene_search::Explanation;

use super::{Fetch, HitView, Stored, prefetch, write_hit_list};
use crate::OpenShard;
use crate::aggs::metric::top_hits::TopHitsResult;
use crate::fetch::docvalue_fields::HitField;
use crate::fetch::highlight::Plan;
use crate::fetch::hit_meta::HitMeta;
use crate::fetch::source_filter::SourceFilter;
use crate::search::{Hit, Hits};

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
    render_picked(
        shard,
        index,
        node_id,
        &hits,
        &explanations,
        (
            &result.source,
            &result.fields,
            result.highlight.as_ref(),
            HitMeta::default(),
        ),
        reserve,
    )
}

/// Each hit at `positions` of `hits` (a shard's top-level result) written as
/// one JSON object, in the order given, as the response writes it: the
/// coordinator's fetch of the hits it kept from a merge of several shards.
///
/// # Errors
///
/// An error when no position is given, when one lies beyond the result, or
/// when a hit cannot be fetched or written.
pub fn render_selected_result_hits<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    index: &str,
    node_id: &str,
    hits: &Hits,
    positions: &[usize],
    reserve: &mut dyn FnMut(usize) -> io::Result<()>,
) -> io::Result<Vec<Vec<u8>>> {
    if positions.is_empty() {
        return Err(io::Error::other("no hit selected"));
    }
    let explained = !hits.explanations.is_empty();
    // ALLOC: one hit and explanation a position, at most the result window.
    let mut picked = Vec::with_capacity(positions.len());
    // ALLOC: one explanation a position when explained.
    let mut explanations = Vec::new();
    for &at in positions {
        picked.push(
            hits.hits
                .get(at)
                .ok_or_else(|| io::Error::other("a selected hit beyond the result"))?
                .clone(),
        );
        if explained {
            explanations.push(
                hits.explanations
                    .get(at)
                    .ok_or_else(|| io::Error::other("a selected hit without its explanation"))?
                    .clone(),
            );
        }
    }
    render_picked(
        shard,
        index,
        node_id,
        &picked,
        &explanations,
        (
            &hits.source,
            &hits.fields,
            hits.highlight.as_ref(),
            hits.meta,
        ),
        reserve,
    )
}

/// What each picked hit is written with: its `_source` filter, its fields
/// and its highlight plan.
type Writing<'a> = (&'a SourceFilter, &'a [HitField], Option<&'a Plan>, HitMeta);

/// Writes each of `hits`, whose explanations are `explanations` when asked.
fn render_picked<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    index: &str,
    node_id: &str,
    hits: &[Hit],
    explanations: &[Explanation],
    (source, fields, highlight, meta): Writing<'_>,
    reserve: &mut dyn FnMut(usize) -> io::Result<()>,
) -> io::Result<Vec<Vec<u8>>> {
    prefetch(shard, &[hits])?;
    let mut fetch = Fetch {
        stored: Stored::new(reserve),
        scratch: 0,
    };
    // ALLOC: one buffer a position, each the hit's own bytes.
    let mut rendered = Vec::with_capacity(hits.len());
    for (i, hit) in hits.iter().enumerate() {
        let view = HitView {
            hits: std::slice::from_ref(hit),
            source,
            fields,
            explanations: explanations.get(i..=i).unwrap_or_default(),
            highlight,
            meta,
        };
        // ALLOC: the hit's JSON, bounded by its source.
        let mut out = Vec::new();
        write_hit_list(&mut out, shard, index, node_id, &view, &mut fetch)?;
        rendered.push(out);
    }
    Ok(rendered)
}
