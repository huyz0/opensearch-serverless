//! `ForwardedSearchResponse` as the Java shell reads it: one shard's total
//! hits and the single-value metric aggregations it was asked for.

use super::aggs::{AggAnswer, write_answers};
use crate::error::WireError;
use crate::stream::Output;

/// The most bytes of an answer; one past it is refused whole, because a page
/// cut short would be a wrong page.
pub const MAX_BYTES: usize = 16 << 20;

/// One hit of a shard, with the fields the coordinator needs to merge and to
/// render it. Every other field of Java's `SearchHit` is written empty.
#[derive(Debug, Clone, Copy)]
pub struct Hit<'a> {
    /// The document id.
    pub id: &'a str,
    /// The score.
    pub score: f32,
    /// The document version.
    pub version: i64,
    /// The sequence number.
    pub seq_no: i64,
    /// The primary term.
    pub primary_term: i64,
    /// The stored `_source` as written.
    pub source: &'a [u8],
}

/// One shard's answer.
#[derive(Debug, Clone, Copy)]
pub struct Response {
    /// How many documents matched in the shard.
    pub total: i64,
    /// Whether `total` is a lower bound (`GREATER_THAN_OR_EQUAL_TO`).
    pub total_is_lower_bound: bool,
    /// The best score, `NaN` when no score was computed.
    pub max_score: f32,
    /// Whether the search timed out.
    pub timed_out: bool,
    /// Whether it stopped early at `terminate_after`, when asked.
    pub terminated_early: Option<bool>,
}

fn float(out: &mut Output<'_>, value: f32) -> Result<(), WireError> {
    out.bytes(&value.to_bits().to_be_bytes())
}

fn optional_bool(out: &mut Output<'_>, value: Option<bool>) -> Result<(), WireError> {
    out.byte(match value {
        Some(false) => 0,
        Some(true) => 1,
        None => 2,
    })
}

/// `SearchHit.writeTo`, with no nested identity, explanation, fields,
/// highlights, sort values, matched queries, shard target or inner hits.
fn hit(out: &mut Output<'_>, hit: &Hit<'_>) -> Result<(), WireError> {
    float(out, hit.score)?;
    // `writeOptionalText`: an int length, then UTF-8.
    let id = hit.id.as_bytes();
    out.int(i32::try_from(id.len()).map_err(|_| WireError::TooLarge {
        what: "a hit id",
        limit: u64::from(i32::MAX.unsigned_abs()),
        actual: id.len() as u64,
    })?)?;
    out.bytes(id)?;
    out.bool(false)?; // nested identity
    out.long(hit.version)?;
    out.zlong(hit.seq_no)?;
    out.vlong(hit.primary_term)?;
    out.bytes_reference(hit.source)?;
    out.bool(false)?; // explanation
    out.vint(0)?; // document fields
    out.vint(0)?; // meta fields
    out.vint(0)?; // highlight fields
    out.vint(0)?; // sort values: formatted
    out.vint(0)?; // sort values: raw
    out.vint(0)?; // matched queries
    out.bool(false)?; // shard target
    out.vint(0) // inner hits
}

impl Response {
    /// `ForwardedSearchResponse.writeTo` for `hits` in the shard's order and
    /// the shard's unreduced `aggregations`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit; use
    /// [`MAX_BYTES`] for an answer.
    pub fn write(
        &self,
        hits: &[Hit<'_>],
        aggregations: &[AggAnswer<'_>],
        out: &mut Output<'_>,
    ) -> Result<(), WireError> {
        out.vlong(self.total)?;
        out.count(hits.len())?;
        for h in hits {
            hit(out, h)?;
        }
        if aggregations.is_empty() {
            out.bool(false)?;
        } else {
            out.bool(true)?;
            write_answers(aggregations, out)?;
        }
        out.bool(self.total_is_lower_bound)?;
        float(out, self.max_score)?;
        out.bool(self.timed_out)?;
        optional_bool(out, self.terminated_early)
    }
}
