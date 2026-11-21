//! A search response's `aggregations`: `Aggregations.toXContent` and each
//! metric's `doXContentBody` (server/src/main/java/org/opensearch/search/aggregations/),
//! at OpenSearch 3.10.0, listed in the order [`hash_map_order`] gives.
//!
//! A metric with no value is `null`, as OpenSearch writes it (a `sum` is
//! `0.0`, a count `0`); a date field's value is also written as text under
//! `value_as_string` (`min_as_string` and so on for `stats`).

use std::io::{self, Write};

use oss_rest::write_json_str;

use crate::aggs::bucket::date_histogram_render::write_date_histogram;
use crate::aggs::bucket::filters_render::write_filters;
use crate::aggs::bucket::histogram_render::write_histogram;
use crate::aggs::bucket::keyword_terms::write_keyword_terms;
use crate::aggs::bucket::range_render::write_range;
use crate::aggs::bucket::terms_render::write_terms;
use crate::aggs::metric::date_text::write_date_of_double;
use crate::aggs::metric::double::write_java_double;
use crate::aggs::metric::hash_order::hash_map_order;
use crate::aggs::metric::percentiles::write_percentiles;
use crate::aggs::metric::top_hits::TopHitsResult;
use crate::aggs::partial::{Format, Partial, Value};

/// Writes a `top_hits` result's body: `"hits":{..}`, with its hits fetched.
pub type HitsWriter<'a> = dyn FnMut(&mut dyn Write, &str, &TopHitsResult) -> io::Result<()> + 'a;

/// Writes `,"aggregations":{..}` for `parts`, or nothing when there are none,
/// for results that hold no `top_hits`.
pub fn write_aggregations(out: &mut dyn Write, parts: &[Partial]) -> io::Result<()> {
    write_aggregations_with(out, parts, &mut |_, _, _| {
        Err(io::Error::other(
            "a top_hits result written without its hits",
        ))
    })
}

/// Writes `,"aggregations":{..}` for `parts`, or nothing when there are none;
/// a `top_hits` result's body is `hits`' to write, given its name.
pub fn write_aggregations_with(
    out: &mut dyn Write,
    parts: &[Partial],
    hits: &mut HitsWriter<'_>,
) -> io::Result<()> {
    if parts.is_empty() {
        return Ok(());
    }
    // ALLOC: one entry an aggregation, bounded by the request's.
    let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    out.write_all(b",\"aggregations\":{")?;
    for (i, at) in hash_map_order(&names).into_iter().enumerate() {
        let Some(part) = parts.get(at) else { continue };
        if i > 0 {
            out.write_all(b",")?;
        }
        write_json_str(out, &part.name)?;
        out.write_all(b":{")?;
        match &part.value {
            Value::TopHits(top) => hits(out, &part.name, top)?,
            value => write_body(out, part.format, value)?,
        }
        out.write_all(b"}")?;
    }
    out.write_all(b"}")
}

/// `"key":` then `value` as a double, or `null` when `present` is false.
fn number(out: &mut dyn Write, key: &str, value: f64, present: bool) -> io::Result<()> {
    write!(out, "\"{key}\":")?;
    if present {
        write_java_double(out, value)
    } else {
        out.write_all(b"null")
    }
}

/// `,"key":"<date>"` for a date-formatted value.
fn as_string(out: &mut dyn Write, key: &str, value: f64) -> io::Result<()> {
    write!(out, ",\"{key}\":\"")?;
    write_date_of_double(out, value)?;
    out.write_all(b"\"")
}

/// One metric's `doXContentBody`.
pub(crate) fn write_body(out: &mut dyn Write, format: Format, value: &Value) -> io::Result<()> {
    let dated = format == Format::Date;
    match *value {
        Value::Cardinality(ref sketch) => {
            // `InternalCardinality.getValue`: 0 when no value was seen.
            write!(
                out,
                "\"value\":{}",
                sketch.as_ref().map_or(0, |s| s.cardinality())
            )
        }
        Value::Min(v) | Value::Max(v) => {
            // `InternalMin`/`InternalMax`: no value is an infinity.
            let present = !v.is_infinite();
            number(out, "value", v, present)?;
            if present && dated {
                as_string(out, "value_as_string", v)?;
            }
            Ok(())
        }
        Value::Sum(v) => {
            number(out, "value", v, true)?;
            if dated {
                as_string(out, "value_as_string", v)?;
            }
            Ok(())
        }
        Value::Avg { sum, count } => {
            let avg = avg(sum, count);
            number(out, "value", avg, count != 0)?;
            if count != 0 && dated {
                as_string(out, "value_as_string", avg)?;
            }
            Ok(())
        }
        Value::Stats {
            count,
            sum,
            min,
            max,
        } => {
            write!(out, "\"count\":{count},")?;
            if count == 0 {
                return out.write_all(b"\"min\":null,\"max\":null,\"avg\":null,\"sum\":0.0");
            }
            let avg = avg(sum, count);
            number(out, "min", min, true)?;
            out.write_all(b",")?;
            number(out, "max", max, true)?;
            out.write_all(b",")?;
            number(out, "avg", avg, true)?;
            out.write_all(b",")?;
            number(out, "sum", sum, true)?;
            if dated {
                as_string(out, "min_as_string", min)?;
                as_string(out, "max_as_string", max)?;
                as_string(out, "avg_as_string", avg)?;
                as_string(out, "sum_as_string", sum)?;
            }
            Ok(())
        }
        Value::ValueCount(n) => write!(out, "\"value\":{n}"),
        Value::Percentiles {
            ranks,
            ref keys,
            keyed,
            ref digest,
            ..
        } => write_percentiles(out, format, ranks, keys, keyed, digest.as_ref()),
        Value::Terms(ref result) => write_terms(out, format, result),
        Value::Histogram(ref result) => write_histogram(out, format, result),
        Value::DateHistogram(ref result) => write_date_histogram(out, format, result),
        Value::KeywordTerms(ref result) => write_keyword_terms(out, result),
        Value::Range(ref result) => write_range(out, format, result),
        Value::Filters(ref result) => write_filters(out, result),
        // Written by the caller, which fetches the hits.
        Value::TopHits(_) | Value::TopHitsParts(_) => Ok(()),
        // Written already: the hits were fetched when it was made.
        Value::Rendered(ref body) => out.write_all(body),
    }
}

/// `sum / count`, as a double division (`InternalAvg.getValue`).
fn avg(sum: f64, count: u64) -> f64 {
    // ARITH: none; a count is far below 2^53, so the conversion is exact,
    // and a zero count divides to NaN, which is never written.
    #[allow(clippy::cast_precision_loss)]
    let count = count as f64;
    sum / count
}
