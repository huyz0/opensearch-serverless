//! `version` and `seq_no_primary_term` of the fetch phase: a hit's
//! `_version`, `_seq_no` and `_primary_term`, read from the segment's numeric
//! doc values of the same names and written between `_id` and `_score`, in
//! the order `SearchHit.toInnerXContent` writes them. A document without the
//! doc value (a nested document, or one an old writer did not stamp) carries
//! none of it.

use std::io::{self, Write};

use lucene_search::directory_reader::SegmentReader;
use lucene_search::reader::doc_values::get_sorted_numeric;

/// Which of a hit's version fields to write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HitMeta {
    /// `version`: write `_version`.
    pub version: bool,
    /// `seq_no_primary_term`: write `_seq_no` and `_primary_term`.
    pub seq_no: bool,
}

/// The first value of the numeric doc values `field` of `doc`, if it has one.
fn first(segment: &SegmentReader, field: &str, doc: i32) -> io::Result<Option<i64>> {
    let mut values = get_sorted_numeric(segment, field).map_err(io::Error::other)?;
    if !values.advance_exact(doc).map_err(io::Error::other)? {
        return Ok(None);
    }
    values.next_value().map(Some).map_err(io::Error::other)
}

/// Writes `,"_version":v` and `,"_seq_no":s,"_primary_term":t` for the
/// fields `meta` asks for and `doc` of `segment` has.
///
/// # Errors
///
/// A doc values read or the write failing.
pub fn write_meta(
    out: &mut dyn Write,
    meta: HitMeta,
    segment: &SegmentReader,
    doc: i32,
) -> io::Result<()> {
    if meta.version
        && let Some(version) = first(segment, "_version", doc)?
    {
        write!(out, ",\"_version\":{version}")?;
    }
    if meta.seq_no
        && let Some(term) = first(segment, "_primary_term", doc)?
        && let Some(seq_no) = first(segment, "_seq_no", doc)?
    {
        write!(out, ",\"_seq_no\":{seq_no},\"_primary_term\":{term}")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "hit_meta_tests.rs"]
mod tests;
