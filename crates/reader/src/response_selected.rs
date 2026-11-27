//! Selected hits of a `top_hits` result, each written as the response writes
//! it, for a coordinator that merged several shards' results and now fetches
//! the hits it kept.

use std::io;

use oss_blob::BlobStore;

use lucene_search::Explanation;

use super::values::write_id;
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
            &[],
            &[],
            false,
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
    // ALLOC: one list of matched names a position when the query names any.
    let mut matched = Vec::new();
    for &at in positions {
        if let Some(m) = hits.matched.get(at) {
            matched.push(m.clone());
        }
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
            &hits.inner,
            &matched,
            hits.named_scores,
        ),
        reserve,
    )
}

/// What each picked hit is written with: its `_source` filter, its fields
/// and its highlight plan.
type Writing<'a> = (
    &'a SourceFilter,
    &'a [HitField],
    Option<&'a Plan>,
    HitMeta,
    &'a [crate::search_inner_hits::ParentInner],
    &'a [Vec<(String, f32)>],
    bool,
);

/// Writes each of `hits`, whose explanations are `explanations` when asked.
fn render_picked<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    index: &str,
    node_id: &str,
    hits: &[Hit],
    explanations: &[Explanation],
    (source, fields, highlight, meta, inner, matched, named_scores): Writing<'_>,
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
            matched: matched.get(i..=i).unwrap_or_default(),
            named_scores,
            highlight,
            meta,
            inner,
        };
        // ALLOC: the hit's JSON, bounded by its source.
        let mut out = Vec::new();
        write_hit_list(&mut out, shard, index, node_id, &view, &mut fetch)?;
        rendered.push(out);
    }
    Ok(rendered)
}

/// `parts` with every `top_hits` under a bucket fetched and written as
/// `Value::Rendered`, for a response of one shard; `None` when no bucket
/// holds one.
///
/// # Errors
///
/// An error when a hit cannot be fetched or written.
pub(crate) fn render_nested<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    index: &str,
    node_id: &str,
    parts: &[crate::aggs::metric::Partial],
    reserve: &mut dyn FnMut(usize) -> io::Result<()>,
) -> io::Result<Option<Vec<crate::aggs::metric::Partial>>> {
    use crate::aggs::bucket::walk::{any_sub, walk_subs_mut};
    use crate::aggs::partial::Value;
    let top = |v: &Value| matches!(v, Value::TopHits(_));
    if !parts.iter().any(|p| any_sub(&p.value, &top)) {
        return Ok(None);
    }
    // ALLOC: one copy of the aggregations a response holds, once.
    let mut owned = parts.to_vec();
    for part in &mut owned {
        walk_subs_mut::<io::Error>(&mut part.value, &mut |_, value| {
            let Value::TopHits(result) = value else {
                return Ok(());
            };
            let ahead = result.before.len();
            // ALLOC: one position a hit of the bucket, at most the inner result window.
            let positions: Vec<usize> = (ahead..ahead.saturating_add(result.hits.len())).collect();
            let hits = if positions.is_empty() {
                // ALLOC: empty.
                Vec::new()
            } else {
                render_selected_hits(shard, index, node_id, result, &positions, reserve)?
            };
            *value = Value::Rendered(top_hits_body(result.total, result.max_score, &hits));
            Ok(())
        })?;
    }
    Ok(Some(owned))
}

/// The aggregation's `"hits":{..}` body from its total, best score and
/// written hits, as a shard writes it.
pub(crate) fn top_hits_body(total: u64, max_score: Option<f32>, hits: &[Vec<u8>]) -> Vec<u8> {
    // ALLOC: the body, bounded by the bucket's rendered hits.
    let mut out = Vec::new();
    // Writing to a vector cannot fail.
    out.extend_from_slice(b"\"hits\":{");
    let _ = crate::hits_total::write_exact(&mut out, total);
    out.extend_from_slice(b"\"max_score\":");
    let _ = super::write_score(&mut out, max_score);
    out.extend_from_slice(b",\"hits\":[");
    for (i, hit) in hits.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend_from_slice(hit);
    }
    out.extend_from_slice(b"]}");
    out
}

/// One hit of a shard as a forwarded search carries it: the stored fields
/// and the doc values, not yet written as JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The `_id` as a JSON string literal (quoted and escaped).
    pub id_json: Vec<u8>,
    /// The score; `None` when the hit was sorted by fields.
    pub score: Option<f32>,
    /// The `_version`, when the document has one.
    pub version: Option<i64>,
    /// The `_seq_no`, when the document has one.
    pub seq_no: Option<i64>,
    /// The `_primary_term`, when the document has one.
    pub primary_term: Option<i64>,
    /// The `_source` as the hits' `_source` filter writes it; empty when the
    /// filter skips it or the document stored none.
    pub source: Vec<u8>,
}

/// Each of `hits` as a [`Row`], in order; `reserve` grants the buffers a
/// hit's `_id`, `_source` and scratch are read into.
///
/// # Errors
///
/// An error when a hit cannot be fetched.
pub fn fetch_rows<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    hits: &Hits,
    reserve: &mut dyn FnMut(usize) -> io::Result<()>,
) -> io::Result<Vec<Row>> {
    prefetch(shard, &[&hits.hits])?;
    let mut stored = Stored::new(reserve);
    let readers = shard.reader.segment_readers();
    // ALLOC: one row a hit, at most the result window.
    let mut rows = Vec::with_capacity(hits.hits.len());
    for hit in &hits.hits {
        let segment = readers
            .iter()
            .rev()
            .find(|r| r.doc_base <= hit.doc)
            .ok_or_else(|| io::Error::other("a hit outside every segment"))?;
        let doc = hit.doc.saturating_sub(segment.doc_base);
        let infos = segment.field_infos();
        stored.id_field = infos.field_by_name("_id").map(|f| f.number);
        stored.source_field = infos.field_by_name("_source").map(|f| f.number);
        stored.id.clear();
        stored.source.clear();
        segment
            .visit_source_document(doc, &mut stored)
            .map_err(io::Error::other)?;
        if let Some(e) = stored.refused.take() {
            return Err(e);
        }
        // ALLOC: the id's literal, bounded by the stored id.
        let mut id_json = Vec::new();
        write_id(&mut id_json, &stored.id)?;
        // ALLOC: the hit's source, reserved by `stored` as it was read.
        let mut source = Vec::new();
        if !hits.source.skip && !stored.source.is_empty() {
            if hits.source.is_whole() {
                source.extend_from_slice(&stored.source);
            } else {
                (stored.reserve)(crate::fetch::source_filter::scratch_bytes(
                    stored.source.len(),
                ))?;
                crate::fetch::source_filter::write_filtered(
                    &mut source,
                    &hits.source,
                    &stored.source,
                )?;
            }
        }
        // ALLOC: three numbers a hit.
        let mut meta = Vec::new();
        crate::fetch::hit_meta::write_meta(
            &mut meta,
            HitMeta {
                version: true,
                seq_no: true,
            },
            segment,
            doc,
        )?;
        let meta = String::from_utf8_lossy(&meta);
        let (version, seq_no, primary_term) = (
            meta_value(&meta, "_version"),
            meta_value(&meta, "_seq_no"),
            meta_value(&meta, "_primary_term"),
        );
        rows.push(Row {
            id_json,
            score: hit.score,
            version,
            seq_no,
            primary_term,
            source,
        });
    }
    Ok(rows)
}

/// The number after `,"name":` in what `write_meta` wrote, when it wrote it.
pub(super) fn meta_value(meta: &str, name: &str) -> Option<i64> {
    // ALLOC: one short key, once a field of a hit.
    let after = meta.split_once(&format!(",\"{name}\":"))?.1;
    after.split(',').next()?.parse().ok()
}
