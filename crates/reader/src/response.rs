//! A search's response body, written as hits are fetched: the fetch phase.
//!
//! Ported from `SearchResponse#innerToXContent`, `SearchHits#toXContent`
//! and `SearchHit#toInnerXContent` (server/src/main/java/org/opensearch/search/),
//! with each hit's `_id` and `_source` read from its stored fields as
//! `FetchPhase` reads them and the id decoded by `Uid.decodeId`
//! (server/src/main/java/org/opensearch/index/mapper/Uid.java), at OpenSearch
//! 3.10.0. One hit's stored fields are held at a time, in a buffer reused
//! for the next; the body goes to the writer as it is made. A JSON
//! `_source` is copied as stored, as OpenSearch copies a source whose type
//! is the response's.

use std::io::{self, Write};

use lucene_codecs::stored_fields::{StoredFieldVisitor, VisitStatus};
use oss_blob::BlobStore;
use oss_rest::write_json_str;

use crate::OpenShard;
use crate::aggs::metric::{Value, write_aggregations_with};
use crate::fetch::docvalue_fields::{self, HitField};
use crate::fetch::highlight::{self, Highlighters, Plan};
use crate::fetch::hit_meta::{self, HitMeta};
use crate::fetch::source_filter::SourceFilter;
use crate::fetch::{explain, source_filter};
use crate::search::{Hit, Hits, SortValue};
use lucene_search::Explanation;

/// A hit's `_id` and `_source`, copied out of its stored fields into buffers
/// that grow only by what `reserve` grants first.
struct Stored<'a> {
    id_field: Option<i32>,
    source_field: Option<i32>,
    id: Vec<u8>,
    source: Vec<u8>,
    reserve: &'a mut dyn FnMut(usize) -> io::Result<()>,
    refused: Option<io::Error>,
}

impl<'a> Stored<'a> {
    fn new(reserve: &'a mut dyn FnMut(usize) -> io::Result<()>) -> Self {
        Self {
            id_field: None,
            source_field: None,
            // ALLOC: none until a field is copied, and then what `reserve`
            // granted.
            id: Vec::new(),
            // ALLOC: as above.
            source: Vec::new(),
            reserve,
            refused: None,
        }
    }
}

impl StoredFieldVisitor for Stored<'_> {
    fn needs_field(
        &mut self,
        field_number: i32,
    ) -> lucene_codecs::stored_fields::Result<VisitStatus> {
        let wanted = Some(field_number) == self.id_field || Some(field_number) == self.source_field;
        Ok(if wanted {
            VisitStatus::Yes
        } else {
            VisitStatus::No
        })
    }

    fn binary_field(
        &mut self,
        field_number: i32,
        value: &[u8],
    ) -> lucene_codecs::stored_fields::Result<()> {
        let into = if Some(field_number) == self.id_field {
            &mut self.id
        } else {
            &mut self.source
        };
        into.clear();
        if let Some(more) = value.len().checked_sub(into.capacity()).filter(|n| *n > 0) {
            if let Err(e) = (self.reserve)(more) {
                self.refused = Some(e);
                return Ok(());
            }
            // ALLOC: the field's bytes, reserved just above.
            into.reserve_exact(value.len());
        }
        into.extend_from_slice(value);
        Ok(())
    }
}

/// Writes the response for `hits` of `shard` of index `index`, fetching
/// each hit's stored fields in turn. The buffers holding a hit's `_id` and
/// `_source` grow to the largest in the window, and only by what `reserve`
/// grants; a refusal ends the response with an error.
pub fn write_response<S: BlobStore + 'static>(
    out: &mut dyn Write,
    shard: &OpenShard<S>,
    index: &str,
    node_id: &str,
    hits: &Hits,
    took_ms: u64,
    reserve: &mut dyn FnMut(usize) -> io::Result<()>,
) -> io::Result<()> {
    write_response_with(out, shard, index, node_id, hits, took_ms, None, reserve)
}

/// [`write_response`] for a search of a point in time, whose response names
/// the view first as `pit_id`.
#[allow(clippy::too_many_arguments)]
pub fn write_response_with<S: BlobStore + 'static>(
    out: &mut dyn Write,
    shard: &OpenShard<S>,
    index: &str,
    node_id: &str,
    hits: &Hits,
    took_ms: u64,
    pit_id: Option<&str>,
    reserve: &mut dyn FnMut(usize) -> io::Result<()>,
) -> io::Result<()> {
    out.write_all(b"{")?;
    if let Some(id) = pit_id {
        out.write_all(b"\"pit_id\":")?;
        write_json_str(out, id)?;
        out.write_all(b",")?;
    }
    write!(out, "\"took\":{took_ms},\"timed_out\":false")?;
    if let Some(early) = hits.terminated_early {
        write!(out, ",\"terminated_early\":{early}")?;
    }
    write!(
        out,
        ",\"_shards\":{{\"total\":1,\"successful\":1,\
         \"skipped\":0,\"failed\":0}},\"hits\":{{"
    )?;
    if hits.tracked {
        write!(
            out,
            "\"total\":{{\"value\":{},\"relation\":\"{}\"}},",
            hits.total,
            if hits.exact { "eq" } else { "gte" }
        )?;
    }
    out.write_all(b"\"max_score\":")?;
    write_score(out, hits.max_score)?;
    out.write_all(b",\"hits\":[")?;
    // ALLOC: one entry the hits and each `top_hits`, bounded by the request's.
    let mut lists: Vec<&[Hit]> = vec![&hits.hits];
    lists.extend(hits.aggregations.iter().filter_map(|p| match &p.value {
        Value::TopHits(top) => Some(top.hits.as_slice()),
        _ => None,
    }));
    // A `top_hits` under a bucket is fetched and written before the list
    // of hits, through its own prefetch.
    let nested = selected::render_nested(shard, index, node_id, &hits.aggregations, reserve)?;
    let aggregations: &[crate::aggs::metric::Partial] =
        nested.as_deref().unwrap_or(&hits.aggregations);
    prefetch(shard, &lists)?;
    let mut fetch = Fetch {
        stored: Stored::new(reserve),
        scratch: 0,
    };
    write_hit_list(out, shard, index, node_id, &hits.view(), &mut fetch)?;
    out.write_all(b"]}")?;
    write_aggregations_with(out, aggregations, &mut |out, _, top| {
        write!(
            out,
            "\"hits\":{{\"total\":{{\"value\":{},\"relation\":\"eq\"}},\"max_score\":",
            top.total
        )?;
        write_score(out, top.max_score)?;
        out.write_all(b",\"hits\":[")?;
        write_hit_list(out, shard, index, node_id, &top.view(), &mut fetch)?;
        out.write_all(b"]}")
    })?;
    out.write_all(b"}")
}

/// `StoredFields.prefetch` of the fetch phase: every hit's chunk of stored
/// fields, of every list the response writes, is announced a segment at a
/// time, so the chunks a cold segment must read arrive as one batch rather
/// than one a hit or one a list.
fn prefetch<S: BlobStore + 'static>(shard: &OpenShard<S>, lists: &[&[Hit]]) -> io::Result<()> {
    let readers = shard.reader.segment_readers();
    // ALLOC: one list a segment and one doc id a hit, at most the result
    // window and the `top_hits` windows.
    let mut announced: Vec<Vec<i32>> = vec![Vec::new(); readers.len()];
    for hit in lists.iter().flat_map(|list| list.iter()) {
        if let Some(at) = readers.iter().rposition(|r| r.doc_base <= hit.doc)
            && let (Some(docs), Some(r)) = (announced.get_mut(at), readers.get(at))
        {
            docs.push(hit.doc.saturating_sub(r.doc_base));
        }
    }
    for (segment, docs) in readers.iter().zip(&mut announced) {
        // A document in several lists is announced once.
        docs.sort_unstable();
        docs.dedup();
        if !docs.is_empty() {
            segment
                .prefetch_stored_documents(docs)
                .map_err(io::Error::other)?;
        }
    }
    Ok(())
}

/// What one list of hits is written from: the hits and how each is shown.
pub(crate) struct HitView<'a> {
    pub hits: &'a [Hit],
    pub source: &'a SourceFilter,
    pub fields: &'a [HitField],
    pub explanations: &'a [Explanation],
    pub highlight: Option<&'a Plan>,
    pub meta: HitMeta,
}

/// The buffers every list of a response's hits is fetched through: one
/// hit's stored fields at a time, and the scratch filtering and
/// highlighting hold, reserved up to the most any hit has needed.
struct Fetch<'a> {
    stored: Stored<'a>,
    scratch: usize,
}

/// Writes the hits of `view`, comma separated, fetching each hit's stored
/// fields in turn.
fn write_hit_list<S: BlobStore + 'static>(
    out: &mut dyn Write,
    shard: &OpenShard<S>,
    index: &str,
    node_id: &str,
    view: &HitView<'_>,
    fetch: &mut Fetch<'_>,
) -> io::Result<()> {
    let Fetch { stored, scratch } = fetch;
    let readers = shard.reader.segment_readers();
    let explained = view.explanations.len() == view.hits.len() && !view.hits.is_empty();
    let mut highlighters = view.highlight.map(Highlighters::new).transpose()?;
    for (i, hit) in view.hits.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        let segment = readers
            .iter()
            .rev()
            .find(|r| r.doc_base <= hit.doc)
            .ok_or_else(|| io::Error::other("a hit outside every segment"))?;
        let infos = segment.field_infos();
        stored.id_field = infos.field_by_name("_id").map(|f| f.number);
        stored.source_field = infos.field_by_name("_source").map(|f| f.number);
        stored.id.clear();
        stored.source.clear();
        segment
            .visit_source_document(hit.doc.saturating_sub(segment.doc_base), stored)
            .map_err(io::Error::other)?;
        if let Some(e) = stored.refused.take() {
            return Err(e);
        }
        out.write_all(b"{")?;
        if explained {
            write!(out, "\"_shard\":\"[{index}][{}]\",\"_node\":", shard.shard)?;
            write_json_str(out, node_id)?;
            out.write_all(b",")?;
        }
        out.write_all(b"\"_index\":")?;
        write_json_str(out, index)?;
        out.write_all(b",\"_id\":")?;
        write_id(out, &stored.id)?;
        hit_meta::write_meta(
            out,
            view.meta,
            segment,
            hit.doc.saturating_sub(segment.doc_base),
        )?;
        out.write_all(b",\"_score\":")?;
        write_score(out, hit.score)?;
        if !view.source.skip && stored.source_field.is_some() && !stored.source.is_empty() {
            if stored.source.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
                return Err(io::Error::other("a _source that is not JSON"));
            }
            out.write_all(b",\"_source\":")?;
            if view.source.is_whole() {
                out.write_all(&stored.source)?;
            } else {
                let need = source_filter::scratch_bytes(stored.source.len());
                if let Some(more) = need.checked_sub(*scratch).filter(|n| *n > 0) {
                    (stored.reserve)(more)?;
                    *scratch = need;
                }
                source_filter::write_filtered(out, view.source, &stored.source)?;
            }
        }
        docvalue_fields::write_fields(
            out,
            view.fields,
            segment,
            hit.doc.saturating_sub(segment.doc_base),
        )?;
        if let (Some(h), Some(plan)) = (highlighters.as_mut(), view.highlight) {
            let need = highlight::scratch_for(stored.source.len(), plan);
            if need > *scratch {
                (stored.reserve)(need.saturating_sub(*scratch))?;
                *scratch = need;
            }
            h.write(out, &stored.source)?;
        }
        if !hit.sort.is_empty() {
            out.write_all(b",\"sort\":[")?;
            for (k, value) in hit.sort.iter().enumerate() {
                if k > 0 {
                    out.write_all(b",")?;
                }
                match value {
                    SortValue::Long(n) => write!(out, "{n}")?,
                    SortValue::Score(s) => write_java_float(out, *s)?,
                    SortValue::Term(Some(t)) => write_json_str(out, &String::from_utf8_lossy(t))?,
                    SortValue::Term(None) => out.write_all(b"null")?,
                }
            }
            out.write_all(b"]")?;
        }
        if let Some(e) = view.explanations.get(i) {
            explain::write_explanation(out, e)?;
        }
        out.write_all(b"}")?;
    }
    Ok(())
}

#[path = "response_selected.rs"]
mod selected;
#[path = "response_values.rs"]
mod values;

pub use selected::{Row, fetch_rows, render_selected_hits, render_selected_result_hits};
use values::write_score;
pub use values::{write_id, write_java_float};

#[cfg(test)]
#[path = "response_hits_tests.rs"]
mod hits_tests;

#[cfg(test)]
#[path = "response_selected_tests.rs"]
mod selected_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn float(f: f32) -> String {
        let mut out = Vec::new();
        write_java_float(&mut out, f).expect("writes");
        String::from_utf8(out).expect("utf-8")
    }

    fn id(bytes: &[u8]) -> String {
        let mut out = Vec::new();
        write_id(&mut out, bytes).expect("writes");
        String::from_utf8(out).expect("utf-8")
    }

    #[test]
    fn floats_print_as_java_prints_them() {
        assert_eq!(float(1.0), "1.0");
        assert_eq!(float(0.0), "0.0");
        assert_eq!(float(-2.0), "-2.0");
        assert_eq!(float(0.526_957_87), "0.52695787");
        assert_eq!(float(1.239_934_2), "1.2399342");
        assert_eq!(float(0.001), "0.001");
        assert_eq!(float(9_999_999.0), "9999999.0");
        assert_eq!(float(1e7), "1.0E7");
        assert_eq!(float(1.5e-4), "1.5E-4");
        assert_eq!(float(-3.25e10), "-3.25E10");
        assert_eq!(float(f32::NAN), "NaN");
        assert_eq!(float(f32::INFINITY), "Infinity");
        assert_eq!(float(f32::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn ids_decode_as_uid_decodes_them() {
        // `Uid.encodeId("1")`: numeric, one digit then the padding nibble.
        assert_eq!(id(&[0xfe, 0x1f]), "\"1\"");
        assert_eq!(id(&[0xfe, 0x12, 0x34]), "\"1234\"");
        assert_eq!(id(&[0xfe, 0x12, 0x3f]), "\"123\"");
        assert_eq!(id(&[0xff, b'a', b'"']), "\"a\\\"\"");
        // Base64 ids: "AAAA" decodes to three zero bytes; the escape byte
        // marks one whose first byte would read as a tag.
        assert_eq!(id(&[0, 0, 0]), "\"AAAA\"");
        assert_eq!(id(&[0xfd, 0xfb, 0xff]), "\"-_8\"");
        assert_eq!(id(&[0x4d, 0x61]), "\"TWE\"");
        assert_eq!(id(&[0x4d]), "\"TQ\"");
        assert_eq!(id(&[]), "\"\"");
    }

    #[test]
    fn the_visitor_keeps_only_the_id_and_the_source() {
        let mut granted = Vec::new();
        let mut grant = |n| {
            granted.push(n);
            Ok(())
        };
        let mut s = Stored::new(&mut grant);
        s.id_field = Some(0);
        s.source_field = Some(1);
        assert!(matches!(s.needs_field(0), Ok(VisitStatus::Yes)));
        assert!(matches!(s.needs_field(1), Ok(VisitStatus::Yes)));
        assert!(matches!(s.needs_field(2), Ok(VisitStatus::No)));
        s.binary_field(0, b"id").expect("id");
        s.binary_field(1, b"{}").expect("source");
        assert_eq!(
            (s.id.as_slice(), s.source.as_slice()),
            (&b"id"[..], &b"{}"[..])
        );
        s.binary_field(1, b"{1}").expect("source");
        s.binary_field(1, b"{}").expect("a shorter source fits");
        drop(s);
        assert_eq!(granted, [2, 2, 1], "each buffer grows by what was granted");
    }

    #[test]
    fn a_refused_reservation_copies_nothing_and_is_kept() {
        let mut refuse = |_| Err(io::Error::other("pool full"));
        let mut s = Stored::new(&mut refuse);
        s.source_field = Some(1);
        s.binary_field(1, b"{}").expect("visits");
        assert!(s.source.is_empty());
        assert_eq!(
            s.refused.map(|e| e.to_string()).as_deref(),
            Some("pool full")
        );
    }
}
