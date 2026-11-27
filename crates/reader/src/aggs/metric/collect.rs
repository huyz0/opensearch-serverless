//! Running a request's metric aggregations on one shard.
//!
//! Each aggregation's field is resolved as `ValuesSourceConfig.resolve` and
//! `ValuesSourceRegistry.getAggregator` resolve it
//! (server/src/main/java/org/opensearch/search/aggregations/support/), at
//! OpenSearch 3.10.0: an unmapped field aggregates nothing, a numeric or
//! date field is read from its doc values, and any other type fails the
//! shard as OpenSearch fails it. The values come from one pass of
//! lucene-aggs' `aggregate_tree` over the query's live matches, segment by
//! segment in document order: OpenSearch's aggregators ported, sums
//! compensated in the same order and `cardinality` sketches written as
//! OpenSearch writes them and t-digests built value by value as OpenSearch
//! builds them, so each state is the one OpenSearch's aggregator
//! builds on a shard searched without concurrent segment slices.

use std::collections::HashMap;

use lucene_aggs::bucket_aggs::{AggNode, AggResult, CardinalityKind, Globals, aggregate_tree};
use lucene_aggs::metric_aggs::{
    MetricState, NEED_COUNT, NEED_MAX, NEED_MIN, NEED_MIN_MAX, NEED_SQ, NEED_SUM, Source, ValueKind,
};
use lucene_search::BooleanQuery;
use lucene_search::field_norms::FieldNorms;
use lucene_search::multi_segment::OpenSegment;
use lucene_search::segment_match::MinScore;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::date_histogram::reduce_results as reduce_dates;
use crate::aggs::bucket::histogram::reduce_results;
use crate::aggs::bucket::{
    composite_collect, date_histogram_collect, date_histogram_spec, filters_collect,
    histogram_collect, histogram_spec, keyword_subs, range_collect, terms_collect, terms_spec,
    with_subs,
};
use crate::aggs::metric::hll::Hll;
use crate::aggs::metric::resolve::bad_request;
pub(crate) use crate::aggs::metric::resolve::{Resolved, resolve, value_kind};
use crate::aggs::metric::script_values;
use crate::aggs::metric::scripted_metric;
use crate::aggs::metric::tdigest::Digest;
use crate::aggs::metric::top_hits::TopHitsResult;
use crate::aggs::metric::{DEFAULT_PRECISION, Metric, MetricAgg};
use crate::aggs::partial::{Format, Partial, Value};
use crate::failure::Failure;
use crate::search::ShardError;

/// What `avg` reads: the sum and the count.
const AVG_NEEDS: u8 = NEED_SUM + NEED_COUNT;

/// What `stats` reads: the count, the sum, the smallest and the largest.
const STATS_NEEDS: u8 = NEED_COUNT + NEED_SUM + NEED_MIN_MAX;

/// What `extended_stats` reads: `stats`' and the sum of squares.
const EXTENDED_NEEDS: u8 = STATS_NEEDS + NEED_SQ;

/// What a metric reads of its field's state.
fn needs(metric: Metric) -> u8 {
    match metric {
        Metric::Min => NEED_MIN,
        Metric::Max => NEED_MAX,
        Metric::Sum => NEED_SUM,
        Metric::Avg => AVG_NEEDS,
        Metric::Stats => STATS_NEEDS,
        Metric::ExtendedStats => EXTENDED_NEEDS,
        Metric::ValueCount
        | Metric::Cardinality
        | Metric::Percentiles
        | Metric::PercentileRanks
        | Metric::TopHits
        | Metric::Terms
        | Metric::SignificantTerms
        | Metric::RareTerms
        | Metric::Histogram
        | Metric::DateHistogram
        | Metric::AutoDateHistogram => NEED_COUNT,
        Metric::Range
        | Metric::DateRange
        | Metric::Filters
        | Metric::AdjacencyMatrix
        | Metric::Filter
        | Metric::Missing
        | Metric::Global
        | Metric::Composite
        | Metric::MultiTerms
        | Metric::Nested
        | Metric::ReverseNested
        | Metric::ScriptedMetric => NEED_COUNT,
    }
}

/// The shard-level result of `agg` from its field's `state`.
pub(crate) fn value_of(agg: &MetricAgg, state: &MetricState) -> Value {
    match agg.metric {
        Metric::Min => Value::Min(state.min_of_mins),
        Metric::Max => Value::Max(state.max_of_maxes),
        Metric::Sum => Value::Sum(state.sum),
        Metric::Avg => Value::Avg {
            sum: state.sum,
            count: state.count,
        },
        Metric::Stats => Value::Stats {
            count: state.count,
            sum: state.sum,
            min: state.min,
            max: state.max,
        },
        Metric::ExtendedStats => Value::ExtendedStats {
            count: state.count,
            sum: state.sum,
            min: state.min,
            max: state.max,
            sum_of_squares: state.sum_of_squares(),
            sigma: agg.sigma,
        },
        Metric::ValueCount => Value::ValueCount(state.count),
        Metric::Cardinality => Value::Cardinality(None),
        Metric::Percentiles | Metric::PercentileRanks => percentiles_of(agg, None),
        // ALLOC: one box a `terms`, bounded by the request's.
        Metric::Terms => Value::Terms(Box::new(terms_collect::empty(&terms_spec(agg)))),
        // ALLOC: one box a `histogram`, bounded by the request's.
        Metric::Histogram => {
            Value::Histogram(Box::new(histogram_collect::empty(&histogram_spec(agg))))
        }
        // ALLOC: one box a `date_histogram`, bounded by the request's.
        Metric::DateHistogram => Value::DateHistogram(Box::new(date_histogram_collect::empty(
            &date_histogram_spec(agg),
        ))),
        Metric::AutoDateHistogram => {
            // ALLOC: one box an `auto_date_histogram`, bounded by the request's.
            Value::AutoDateHistogram(Box::new(crate::aggs::bucket::auto_date::AutoDateResult {
                spec: agg
                    .auto_date
                    .as_deref()
                    .cloned()
                    .unwrap_or(crate::aggs::bucket::auto_date::AutoDateSpec::FALLBACK),
                idx: 0,
                inner: 1,
                // ALLOC: empty.
                buckets: Vec::new(),
                reduced: false,
            }))
        }
        Metric::Range | Metric::DateRange => range_collect::value(agg, &[]),
        Metric::Filters
        | Metric::AdjacencyMatrix
        | Metric::Filter
        | Metric::Missing
        | Metric::Global
        | Metric::Nested
        | Metric::ReverseNested => filters_collect::value(agg, &[]),
        Metric::Composite => composite_collect::empty(agg),
        Metric::MultiTerms => crate::aggs::bucket::multi_collect::empty(agg),
        Metric::SignificantTerms => crate::aggs::bucket::significant_collect::unmapped(
            &crate::aggs::bucket::significant_collect::spec_of(agg),
        ),
        Metric::RareTerms => crate::aggs::bucket::rare_collect::unmapped(
            &crate::aggs::bucket::rare_collect::spec_of(agg),
        ),
        // Filled in by the search once the hits are collected.
        // ALLOC: one box a `top_hits`, bounded by the request's.
        Metric::TopHits => Value::TopHits(Box::new(TopHitsResult::empty())),
        Metric::ScriptedMetric => agg.scripted.as_deref().map_or_else(
            || Value::Min(f64::INFINITY),
            // ALLOC: one box a `scripted_metric`, bounded by the request's.
            |spec| Value::Scripted(Box::new(scripted_metric::empty(spec))),
        ),
    }
}

/// `agg`'s percentiles over `digest`, settled here once so the response
/// and a reduce read it without changing it.
fn percentiles_of(agg: &MetricAgg, digest: Option<Digest>) -> Value {
    Value::Percentiles {
        ranks: agg.metric == Metric::PercentileRanks,
        // ALLOC: one key an asked-for percent, bounded by the request's.
        keys: agg.percentiles.keys.clone(),
        keyed: agg.percentiles.keyed,
        compression: agg.percentiles.compression,
        digest: digest.map(|mut d| {
            d.settle();
            d
        }),
    }
}

/// `CardinalityAggregatorFactory`'s `ExecutionMode.fromString`, run as
/// the factory is built.
fn check_execution_hint(agg: &MetricAgg) -> Result<(), ShardError> {
    match agg.cardinality.execution_hint.as_deref() {
        None => Ok(()),
        Some(hint) if matches!(hint.to_uppercase().as_str(), "DIRECT" | "ORDINALS") => Ok(()),
        Some(hint) => Err(bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("Unknown execution_hint: [{hint}], expected any of [direct, ordinals]"),
        ))),
    }
}

/// The engine's node for a numeric `agg` at `precision` (a `cardinality`'s).
pub(crate) fn node(agg: &MetricAgg, precision: u32, values: ValueKind) -> AggNode {
    // ALLOC: one field name an aggregation, bounded by the request's.
    let field = agg.field.clone();
    // `byte` to `long` and `date` are longs read as they are.
    match agg.metric {
        Metric::Terms => terms_collect::node(&agg.field, &terms_spec(agg), values),
        Metric::Histogram => histogram_collect::node(&agg.field, &histogram_spec(agg), values),
        Metric::DateHistogram => {
            date_histogram_collect::node(&agg.field, &date_histogram_spec(agg))
        }
        Metric::Range | Metric::DateRange => range_collect::node(agg, values),
        Metric::Composite | Metric::MultiTerms => {
            unreachable!("a composite's node is made by composite_collect::node")
        }
        Metric::Cardinality => AggNode::Cardinality {
            field,
            kind: CardinalityKind::Numeric(values),
            precision: Some(precision),
        },
        Metric::Percentiles | Metric::PercentileRanks => AggNode::Percentiles {
            field,
            missing: None,
            kind: values,
            compression: agg.percentiles.compression,
            hdr: false,
        },
        metric => AggNode::Metric {
            field,
            missing: None,
            kind: values,
            source: Source::DocValues,
            needs: needs(metric),
            script: None,
        },
    }
}

fn engine_error(e: &dyn std::fmt::Display) -> ShardError {
    ShardError {
        status: 500,
        // ALLOC: error path; one message.
        failure: Failure::opensearch("exception", e.to_string()),
    }
}

/// The shard-level result of `agg` from the engine's `result`.
pub(crate) fn value_from(agg: &MetricAgg, result: Option<AggResult>) -> Result<Value, ShardError> {
    match result {
        Some(AggResult::Metric(states)) => Ok(value_of(
            agg,
            states.first().unwrap_or(&MetricState::default()),
        )),
        Some(AggResult::CardinalitySketch(sketches)) => match sketches.first().map(Vec::as_slice) {
            None | Some([]) => Ok(Value::Cardinality(None)),
            Some(bytes) => Hll::read(bytes)
                .map(|h| Value::Cardinality(Some(h)))
                .ok_or_else(|| engine_error(&"an unreadable cardinality sketch")),
        },
        // A number field never holds a NaN, which a digest refuses.
        Some(AggResult::Digests {
            digests,
            nan: false,
        }) => Ok(percentiles_of(
            agg,
            digests
                .into_iter()
                .next()
                .flatten()
                .map(Digest::from_engine),
        )),
        Some(AggResult::NumericTerms { buckets, .. }) if agg.metric == Metric::Terms => {
            let (other, kept) = buckets.into_iter().next().unwrap_or_default();
            // ALLOC: the kept buckets of one `terms`, at most its shard size.
            let mut result = terms_collect::from_engine(&terms_spec(agg), other, kept);
            if agg.subs.is_empty() {
                result.apply_shard_min();
            }
            // ALLOC: one box a `terms`, bounded by the request's.
            Ok(Value::Terms(Box::new(result)))
        }
        Some(AggResult::Histogram { buckets, .. }) if agg.metric == Metric::Histogram => {
            let spec = histogram_spec(agg);
            let result = histogram_collect::from_engine(
                &spec,
                buckets.first().map_or(&[][..], Vec::as_slice),
            );
            // The answer's gaps count against the limit as the buckets do.
            reduce_results(&[&result], BUCKET_LIMIT)
                .map_err(|made| terms_collect::too_many_buckets(made, BUCKET_LIMIT))?;
            // ALLOC: one box a `histogram`, bounded by the request's.
            Ok(Value::Histogram(Box::new(result)))
        }
        Some(AggResult::DateHistogram { buckets, .. }) if agg.metric == Metric::DateHistogram => {
            let spec = date_histogram_spec(agg);
            let result = date_histogram_collect::from_engine(
                &spec,
                buckets.first().map_or(&[][..], Vec::as_slice),
            );
            // The answer's gaps count against the limit as the buckets do.
            reduce_dates(&[&result], BUCKET_LIMIT)
                .map_err(|made| terms_collect::too_many_buckets(made, BUCKET_LIMIT))?;
            // ALLOC: one box a `date_histogram`, bounded by the request's.
            Ok(Value::DateHistogram(Box::new(result)))
        }
        _ => Err(engine_error(&"an aggregation result of another kind")),
    }
}

/// Runs `aggs` over `query`'s live matches in `segments` of `shard`, in one
/// pass: one result each, in request order. A field the aggregation cannot
/// read, or an option the shard refuses, fails the shard with OpenSearch's
/// status and exception, in OpenSearch's order: every factory is built
/// (the field, then a `cardinality`'s hint) before any aggregator (its
/// precision).
pub fn collect<S: BlobStore + 'static>(
    aggs: &[MetricAgg],
    shard: &OpenShard<S>,
    segments: &[OpenSegment<'_>],
    query: &BooleanQuery,
    filters: &[Vec<BooleanQuery>],
    norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
) -> Result<Vec<Partial>, ShardError> {
    if aggs.is_empty() {
        // ALLOC: empty.
        return Ok(Vec::new());
    }
    // ALLOC: one entry an aggregation, bounded by the request's.
    let mut resolved = Vec::with_capacity(aggs.len());
    for agg in aggs {
        if let Some(refusal) = agg
            .histogram
            .as_deref()
            .and_then(histogram_collect::interval_refusal)
        {
            return Err(refusal);
        }
        keyword_subs::check_order(agg)?;
        resolved.push(resolve(agg, shard)?);
        crate::aggs::bucket::check_limits(agg)?;
        if agg.metric == Metric::Cardinality {
            check_execution_hint(agg)?;
        }
    }
    // ALLOC: as above.
    let mut nodes = Vec::with_capacity(aggs.len());
    // ALLOC: one entry a keyword `terms`, bounded by the request's.
    let mut ords = HashMap::new();
    // ALLOC: one entry a `scripted_metric`, bounded by the request's.
    let mut scripted: HashMap<usize, scripted_metric::Prepared> = HashMap::new();
    for (i, (agg, r)) in aggs.iter().zip(&resolved).enumerate() {
        let precision = match agg.metric {
            Metric::Cardinality => agg
                .cardinality
                .precision()
                .map_err(|m| bad_request(Failure::plain("illegal_argument_exception", m)))?,
            _ => DEFAULT_PRECISION,
        };
        let mut queries = filters_collect::Queries::new(filters.get(i));
        match r {
            Resolved::Numeric(_) => {
                let mut made = node(agg, precision, value_kind(agg, shard));
                if let (Some(script), AggNode::Metric { script: slot, .. }) =
                    (agg.script.as_deref(), &mut made)
                {
                    *slot = Some(script_values::values(&shard.meta.mapping, script)?);
                }
                if let (Some(spec), AggNode::Metric { script: slot, .. }) =
                    (scripted_metric::spec_of(agg), &mut made)
                {
                    let prepared = scripted_metric::prepare(spec, &shard.meta.mapping)?;
                    *slot = Some(prepared.source());
                    scripted.insert(i, prepared);
                }
                with_subs::attach(&mut made, agg, shard, &mut ords, &mut queries)?;
                nodes.push(made);
            }
            Resolved::Keyword => {
                nodes.push(keyword_subs::node(agg, shard, &mut ords, &mut queries)?)
            }
            Resolved::Queries => {
                let mut made = filters_collect::node(agg, queries.take_own(agg));
                with_subs::attach(&mut made, agg, shard, &mut ords, &mut queries)?;
                nodes.push(made);
            }
            Resolved::Composite(true) => {
                nodes.push(composite_collect::node(
                    agg,
                    shard,
                    &mut ords,
                    segments.len(),
                )?);
            }
            Resolved::Unmapped | Resolved::Hits | Resolved::Composite(false) => {}
        }
    }
    let mut results = if nodes.is_empty() {
        // ALLOC: empty.
        Vec::new()
    } else {
        // ALLOC: one index a segment: one slice, the whole shard in order.
        let slices = [(0..segments.len()).collect::<Vec<usize>>()];
        // A `top_hits` under a bucket scores the query, behind no `min_score`.
        let scoring = MinScore {
            min: f32::NEG_INFINITY,
            norms,
        };
        aggregate_tree(
            segments,
            shard.reader.segment_readers(),
            query,
            &nodes,
            &Globals::new(&ords).with_bucket_limit(BUCKET_LIMIT),
            &slices,
            nodes.iter().any(AggNode::needs_scores).then_some(&scoring),
        )
        .map_err(|e| match e {
            lucene_search::Error::TooManyBuckets(made, limit) => {
                terms_collect::too_many_buckets(made, limit)
            }
            e => engine_error(&e),
        })?
        .into_iter()
        .next()
        .unwrap_or_default()
    }
    .into_iter();
    // ALLOC: one result an aggregation, as above.
    let mut parts = Vec::with_capacity(aggs.len());
    for (i, (agg, r)) in aggs.iter().zip(resolved).enumerate() {
        let (format, value) = match r {
            Resolved::Numeric(format) if scripted.contains_key(&i) => {
                results.next();
                let value = match (scripted_metric::spec_of(agg), scripted.get(&i)) {
                    (Some(spec), Some(prepared)) => {
                        // ALLOC: one box a `scripted_metric`, bounded by the request's.
                        Value::Scripted(Box::new(scripted_metric::finish(spec, prepared)?))
                    }
                    _ => value_of(agg, &MetricState::default()),
                };
                (format, value)
            }
            Resolved::Unmapped if with_subs::is_bucket(agg) => {
                (Format::Raw, with_subs::value(agg, shard, None)?)
            }
            Resolved::Unmapped | Resolved::Hits | Resolved::Composite(false) => {
                (Format::Raw, value_of(agg, &MetricState::default()))
            }
            Resolved::Composite(true) if agg.metric == Metric::MultiTerms => (
                Format::Raw,
                crate::aggs::bucket::multi_collect::value(agg, shard, results.next())?,
            ),
            Resolved::Composite(true) => (
                Format::Raw,
                composite_collect::value_from(agg, shard, results.next())?,
            ),
            Resolved::Numeric(format) if with_subs::is_bucket(agg) => {
                (format, with_subs::value(agg, shard, results.next())?)
            }
            Resolved::Numeric(format) => (format, value_from(agg, results.next())?),
            Resolved::Queries => (Format::Raw, with_subs::value(agg, shard, results.next())?),
            Resolved::Keyword => (
                Format::Raw,
                keyword_subs::value(agg, shard, results.next())?,
            ),
        };
        parts.push(Partial {
            // ALLOC: one name an aggregation, as above.
            name: agg.name.clone(),
            format,
            value,
        });
    }
    Ok(parts)
}

#[cfg(test)]
#[path = "collect_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "collect_decimal_tests.rs"]
mod decimal_tests;

#[cfg(test)]
#[path = "collect_half_tests.rs"]
mod half_tests;

#[cfg(test)]
#[path = "collect_scaled_tests.rs"]
mod scaled_tests;
