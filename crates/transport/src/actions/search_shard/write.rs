//! Writing a `ForwardedSearchRequest`: the bytes the Java shell writes for
//! the same request, for the part of the source this node reads and a
//! match-all or absent query.

use super::query::Query;
use super::{Refusal, Request, Source};
use crate::stream::Output;

impl Request {
    /// The request's wire bytes, in the layout [`Request::read`] reads.
    ///
    /// # Errors
    ///
    /// [`Refusal::UnsupportedQuery`] for a query other than match-all, which
    /// this writer does not encode, or the wire's own error for a value that
    /// does not fit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), Refusal> {
        self.parent.write(out)?;
        out.string(&self.index)?;
        out.vint(self.shard)?;
        self.source.write(out)?;
        out.optional_string(self.index_uuid.as_deref())?;
        out.long(self.now_millis)?;
        Ok(())
    }
}

fn float(out: &mut Output<'_>, value: f32) -> Result<(), Refusal> {
    out.bytes(&value.to_bits().to_be_bytes())?;
    Ok(())
}

impl Source {
    /// The source's wire bytes: `SearchSourceBuilder.writeTo` with every
    /// field this node does not run at its default.
    ///
    /// # Errors
    ///
    /// As [`Request::write`].
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), Refusal> {
        if self.aggregations.is_empty() && self.pipelines.is_empty() {
            out.bool(false)?;
        } else {
            out.bool(true)?;
            super::aggs::write_with_pipelines(&self.aggregations, &self.pipelines, out)?;
        }
        out.byte(2)?; // explain: none
        match &self.fetch_source {
            Some((fetch, includes, excludes)) => {
                out.bool(true)?;
                out.bool(*fetch)?;
                out.string_array(&includes.iter().map(String::as_str).collect::<Vec<_>>())?;
                out.string_array(&excludes.iter().map(String::as_str).collect::<Vec<_>>())?;
            }
            None => out.bool(false)?,
        }
        out.bool(false)?; // docvalue_fields
        out.bool(false)?; // stored_fields
        out.vint(self.from)?;
        out.bool(false)?; // highlight
        out.vint(0)?; // indices_boost
        match self.min_score {
            Some(score) => {
                out.bool(true)?;
                float(out, score)?;
            }
            None => out.bool(false)?,
        }
        out.bool(false)?; // post_filter
        match &self.query {
            None => out.bool(false)?,
            Some(node) if matches!(node.query, Query::MatchAll) && node.name.is_none() => {
                out.bool(true)?;
                out.string("match_all")?;
                float(out, node.boost)?;
                out.optional_string(None)?;
            }
            Some(_) => {
                return Err(Refusal::UnsupportedQuery(
                    "a query other than match_all".into(),
                ));
            }
        }
        out.bool(false)?; // rescore
        out.bool(false)?; // script_fields
        out.vint(self.size)?;
        out.bool(false)?; // sort
        out.bool(false)?; // stats
        out.bool(false)?; // suggest
        out.vint(self.terminate_after)?;
        match self.timeout_millis {
            Some(millis) => {
                out.bool(true)?;
                // The largest `TimeUnit` that holds the count exactly; the
                // request does not keep the unit it was written in.
                let (count, unit) = [
                    (86_400_000_i64, 6_u8),
                    (3_600_000, 5),
                    (60_000, 4),
                    (1_000, 3),
                ]
                .into_iter()
                .find(|(per, _)| millis != 0 && millis.checked_rem(*per) == Some(0))
                .map_or((millis, 2), |(per, unit)| {
                    (millis.checked_div(per).unwrap_or(millis), unit)
                });
                out.zlong(count)?;
                out.byte(unit)?;
            }
            None => out.bool(false)?,
        }
        out.bool(false)?; // track_scores
        out.byte(2)?; // version: none
        out.byte(2)?; // seq_no_primary_term: none
        out.vint(0)?; // ext
        out.bool(false)?; // profile
        out.bool(false)?; // search_after
        out.bool(false)?; // slice
        out.bool(false)?; // collapse
        match self.track_total_hits_up_to {
            Some(up_to) => {
                out.bool(true)?;
                out.int(up_to)?;
            }
            None => out.bool(false)?,
        }
        out.bool(false)?; // fields
        out.bool(false)?; // pit
        out.bool(false)?; // search_pipeline source
        out.byte(2)?; // include_named_queries_score: none
        out.bool(false)?; // derived fields object
        out.bool(false)?; // derived fields
        out.bool(false)?; // search_pipeline
        out.bool(false)?; // verbose_pipeline
        Ok(())
    }
}
