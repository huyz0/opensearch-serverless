//! A hit's `inner_hits` as `SearchHit#toXContent` writes them at OpenSearch
//! 3.10.0: under each name the hits of the nested objects that matched, each
//! with the parent's `_index` and `_id`, its `_nested` identity, its score
//! and its sub-source.

use std::io::{self, Write};

use oss_rest::write_json_str;

use super::values::write_score;
use super::write_id;
use crate::fetch::nested_source;
use crate::fetch::source_filter::scratch_bytes;
use crate::search_inner_hits::ParentInner;

/// Writes `,"inner_hits":{...}` for `parent`, whose `_id` and `_source` are
/// `id` and `source`; `scratch` is what `reserve` has granted a source cut so
/// far.
pub(super) fn write_inner_hits(
    out: &mut dyn Write,
    parent: &ParentInner,
    (index, id, source): (&str, &[u8], &[u8]),
    scratch: &mut usize,
    reserve: &mut dyn FnMut(usize) -> io::Result<()>,
) -> io::Result<()> {
    out.write_all(b",\"inner_hits\":{")?;
    for (n, r) in parent.results.iter().enumerate() {
        if n > 0 {
            out.write_all(b",")?;
        }
        write_json_str(out, &r.name)?;
        write!(
            out,
            ":{{\"hits\":{{\"total\":{{\"value\":{},\"relation\":\"eq\"}},\"max_score\":",
            r.total
        )?;
        write_score(out, r.max_score)?;
        out.write_all(b",\"hits\":[")?;
        for (i, (hit, offset)) in r.hits.iter().zip(&r.offsets).enumerate() {
            if i > 0 {
                out.write_all(b",")?;
            }
            out.write_all(b"{\"_index\":")?;
            write_json_str(out, index)?;
            out.write_all(b",\"_id\":")?;
            write_id(out, id)?;
            out.write_all(b",\"_nested\":{\"field\":")?;
            write_json_str(out, &r.path)?;
            write!(out, ",\"offset\":{offset}}},\"_score\":")?;
            write_score(out, hit.score)?;
            if !r.source_skip && !source.is_empty() {
                let need = scratch_bytes(source.len());
                if let Some(more) = need.checked_sub(*scratch).filter(|m| *m > 0) {
                    reserve(more)?;
                    *scratch = need;
                }
                // A source the child is not in writes none, as an update
                // that dropped it would leave.
                // ALLOC: the cut, at most the source whose scratch was reserved.
                let mut cut = Vec::new();
                if nested_source::write_nested(&mut cut, source, &r.path, *offset)? {
                    out.write_all(b",\"_source\":")?;
                    out.write_all(&cut)?;
                }
            }
            super::write_sort(out, &hit.sort)?;
            out.write_all(b"}")?;
        }
        out.write_all(b"]}}")?;
    }
    out.write_all(b"}")
}
