//! A search response's `aggregations`: `Aggregations.toXContent` and each
//! metric's `doXContentBody` (server/src/main/java/org/opensearch/search/aggregations/),
//! at OpenSearch 3.10.0, listed in the order [`hash_map_order`] gives.
//!
//! A metric with no value is `null`, as OpenSearch writes it (a `sum` is
//! `0.0`, a count `0`); a date field's value is also written as text under
//! `value_as_string` (`min_as_string` and so on for `stats`).

use std::io::{self, Write};

use crate::aggs::bucket::composite_render::write_composite;
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
use crate::aggs::metric::typed::{scoped, write_key, write_pipeline_key};
use crate::aggs::partial::{Format, Partial, Value};
use crate::aggs::pipeline::{PipelineAgg, compute, parent};

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
    write_aggregations_typed(out, parts, false, hits)
}

/// [`write_aggregations_with`], each key written as `type#name` when `typed` is set.
pub fn write_aggregations_typed(
    out: &mut dyn Write,
    parts: &[Partial],
    typed: bool,
    hits: &mut HitsWriter<'_>,
) -> io::Result<()> {
    write_aggregations_piped(out, parts, &[], typed, hits)
}

/// [`write_aggregations_typed`] with the sibling pipeline aggregations of the
/// request, each computed over the aggregation its path names and listed
/// beside the others as `Aggregations.toXContent` lists them.
pub fn write_aggregations_piped(
    out: &mut dyn Write,
    parts: &[Partial],
    pipelines: &[PipelineAgg],
    typed: bool,
    hits: &mut HitsWriter<'_>,
) -> io::Result<()> {
    scoped(typed, || write_parts(out, parts, pipelines, typed, hits))
}

fn write_parts(
    out: &mut dyn Write,
    parts: &[Partial],
    all: &[PipelineAgg],
    typed: bool,
    hits: &mut HitsWriter<'_>,
) -> io::Result<()> {
    if parts.is_empty() {
        return Ok(());
    }
    out.write_all(b",\"aggregations\":{")?;
    // `InternalAggregations.reduce` lists the aggregations in a `HashMap`'s
    // order; the sibling pipeline results are added after them, in request
    // order.
    // ALLOC: one entry an aggregation, bounded by the request's.
    let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    for (i, at) in hash_map_order(&names).into_iter().enumerate() {
        let Some(part) = parts.get(at) else {
            continue;
        };
        if i > 0 {
            out.write_all(b",")?;
        }
        write_key(out, &part.name, &part.value, part.format)?;
        out.write_all(b":{")?;
        match &part.value {
            Value::TopHits(top) => hits(out, &part.name, top)?,
            value if !holds_parents(all, &part.name) => write_body(out, part.format, value)?,
            _ => {
                let piped = host_body(part, all, typed)?;
                out.write_all(
                    piped
                        .get(1..piped.len().saturating_sub(1))
                        .unwrap_or_default(),
                )?;
            }
        }
        out.write_all(b"}")?;
    }
    for (i, pipeline) in all.iter().filter(|p| p.host.is_none()).enumerate() {
        if i > 0 || !parts.is_empty() {
            out.write_all(b",")?;
        }
        write_pipeline_key(out, pipeline)?;
        out.write_all(b":{")?;
        if let Some(part) = parts.iter().find(|p| p.name == pipeline.agg) {
            let target = scoped(false, || host_body(part, all, false))?;
            compute::write_result(out, pipeline, &target)?;
        }
        out.write_all(b"}")?;
    }
    out.write_all(b"}")
}

/// Whether a parent pipeline sits in the aggregation `name`.
fn holds_parents(all: &[PipelineAgg], name: &str) -> bool {
    all.iter().any(|p| p.host.as_deref() == Some(name))
}

/// The body of `part` with braces, its parent pipelines applied.
fn host_body(part: &Partial, all: &[PipelineAgg], typed: bool) -> io::Result<Vec<u8>> {
    // ALLOC: the rendered buckets of one aggregation, read back by the pipelines that work on them.
    let mut body = vec![b'{'];
    write_body(&mut body, part.format, &part.value)?;
    body.push(b'}');
    // ALLOC: one entry a parent pipeline of this aggregation.
    let held: Vec<&PipelineAgg> = all
        .iter()
        .filter(|p| p.host.as_deref() == Some(part.name.as_str()))
        // ALLOC: one entry or message bounded by the request's pipelines and names.
        .collect();
    if held.is_empty() {
        Ok(body)
    } else {
        parent::apply(&body, &held, typed)
    }
}

/// `"key":` then `value` as a double, or `null` when `present` is false.
fn number(out: &mut dyn Write, key: &str, value: f64, present: bool) -> io::Result<()> {
    write!(out, "\"{key}\":")?;
    if present && value.is_finite() {
        write_java_double(out, value)
    } else if present {
        // The JSON writer quotes a NaN or an infinity (`QUOTE_NON_NUMERIC_NUMBERS`).
        out.write_all(b"\"")?;
        write_java_double(out, value)?;
        out.write_all(b"\"")
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

/// `,"<key>_as_string":"<date>"` for a date-formatted value.
fn text_of(out: &mut dyn Write, key: &str, value: f64) -> io::Result<()> {
    write!(out, ",\"{key}_as_string\":\"")?;
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
        Value::ExtendedStats {
            count,
            sum,
            min,
            max,
            sum_of_squares,
            sigma,
        } => write_extended(out, dated, count, sum, min, max, sum_of_squares, sigma),
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
        Value::AutoDateHistogram(ref result) => {
            crate::aggs::bucket::auto_date_render::write_auto_date(out, format, result)
        }
        Value::KeywordTerms(ref result) => write_keyword_terms(out, result),
        Value::Significant(ref result) => {
            crate::aggs::bucket::significant::write_significant(out, result)
        }
        Value::Rare(ref result) => crate::aggs::bucket::rare::write_rare(out, result),
        Value::Range(ref result) => write_range(out, format, result),
        Value::Filters(ref result) => write_filters(out, result),
        Value::Composite(ref result) => write_composite(out, result),
        Value::Multi(ref result) => crate::aggs::bucket::multi::write_multi(out, result),
        // Written by the caller, which fetches the hits.
        Value::TopHits(_) | Value::TopHitsParts(_) => Ok(()),
        // Written already: the hits were fetched when it was made.
        Value::Rendered(ref body) => out.write_all(body),
        Value::Scripted(ref result) => super::scripted_metric::write(out, result),
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

/// The variance of the population: `(sumOfSqrs - sum * sum / count) / count`,
/// not below 0 (`InternalExtendedStats.variancePopulation`).
fn variance_population(count: f64, sum: f64, squares: f64) -> f64 {
    let variance = (squares - ((sum * sum) / count)) / count;
    if variance < 0.0 { 0.0 } else { variance }
}

/// The variance of a sample: over `count - 1` (`varianceSampling`).
fn variance_sampling(count: f64, sum: f64, squares: f64) -> f64 {
    let variance = (squares - ((sum * sum) / count)) / (count - 1.0);
    if variance < 0.0 { 0.0 } else { variance }
}

/// `InternalExtendedStats.doXContentBody`.
#[allow(clippy::too_many_arguments)]
fn write_extended(
    out: &mut dyn Write,
    dated: bool,
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
    squares: f64,
    sigma: f64,
) -> io::Result<()> {
    write!(out, "\"count\":{count},")?;
    if count == 0 {
        out.write_all(
            b"\"min\":null,\"max\":null,\"avg\":null,\"sum\":0.0,\"sum_of_squares\":null,\
              \"variance\":null,\"variance_population\":null,\"variance_sampling\":null,\
              \"std_deviation\":null,\"std_deviation_population\":null,\
              \"std_deviation_sampling\":null,\"std_deviation_bounds\":{\"upper\":null,\
              \"lower\":null,\"upper_population\":null,\"lower_population\":null,\
              \"upper_sampling\":null,\"lower_sampling\":null}",
        )?;
        return Ok(());
    }
    // ARITH: none; a count is far below 2^53, so the conversion is exact.
    #[allow(clippy::cast_precision_loss)]
    let n = count as f64;
    let avg = sum / n;
    let population = variance_population(n, sum, squares);
    let sampling = variance_sampling(n, sum, squares);
    let (deviation, deviation_sampling) = (population.sqrt(), sampling.sqrt());
    let bounds = [
        ("upper", avg + deviation * sigma),
        ("lower", avg - deviation * sigma),
        ("upper_population", avg + deviation * sigma),
        ("lower_population", avg - deviation * sigma),
        ("upper_sampling", avg + deviation_sampling * sigma),
        ("lower_sampling", avg - deviation_sampling * sigma),
    ];
    let fields = [
        ("min", min),
        ("max", max),
        ("avg", avg),
        ("sum", sum),
        ("sum_of_squares", squares),
        ("variance", population),
        ("variance_population", population),
        ("variance_sampling", sampling),
        ("std_deviation", deviation),
        ("std_deviation_population", deviation),
        ("std_deviation_sampling", deviation_sampling),
    ];
    for (i, (key, value)) in fields.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        number(out, key, *value, true)?;
        // The text of min, max, avg and sum follows the sum, before the rest.
        if *key == "sum" && dated {
            for (name, v) in [("min", min), ("max", max), ("avg", avg), ("sum", sum)] {
                text_of(out, name, v)?;
            }
        }
    }
    out.write_all(b",\"std_deviation_bounds\":{")?;
    for (i, (key, value)) in bounds.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        number(out, key, *value, true)?;
    }
    out.write_all(b"}")?;
    if dated {
        for (key, value) in &fields[4..] {
            text_of(out, key, *value)?;
        }
        out.write_all(b",\"std_deviation_bounds_as_string\":{")?;
        for (i, (key, value)) in bounds.iter().enumerate() {
            if i > 0 {
                out.write_all(b",")?;
            }
            write!(out, "\"{key}\":\"")?;
            write_date_of_double(out, *value)?;
            out.write_all(b"\"")?;
        }
        out.write_all(b"}")?;
    }
    Ok(())
}
