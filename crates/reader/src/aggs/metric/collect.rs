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
    MetricState, NEED_COUNT, NEED_MAX, NEED_MIN, NEED_MIN_MAX, NEED_SUM, Source, ValueKind,
};
use lucene_search::BooleanQuery;
use lucene_search::multi_segment::OpenSegment;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::histogram::{HistogramSpec, reduce_results};
use crate::aggs::bucket::histogram_collect;
use crate::aggs::bucket::keyword_collect;
use crate::aggs::bucket::terms::TermsSpec;
use crate::aggs::bucket::terms_collect;
use crate::aggs::metric::hll::Hll;
use crate::aggs::metric::tdigest::Digest;
use crate::aggs::metric::top_hits::TopHitsResult;
use crate::aggs::metric::{DEFAULT_PRECISION, Metric, MetricAgg};
use crate::aggs::partial::{Format, Partial, Value};
use crate::failure::Failure;
use crate::mapping::FieldType;
use crate::search::ShardError;

/// What `ValuesSourceConfig` resolved an aggregation's field to.
enum Resolved {
    /// No such field: the aggregation sees no value.
    Unmapped,
    /// A numeric field's doc values, and how its values are shown.
    Numeric(Format),
    /// A keyword field's global ordinals, for a `terms`.
    Keyword,
}

/// The text of `Field [<name>] of type [<type>]`'s type.
fn type_name(ty: &FieldType) -> &str {
    match ty {
        FieldType::Text => "text",
        FieldType::Keyword => "keyword",
        FieldType::Int { name, .. } => name,
        FieldType::Long => "long",
        FieldType::Date => "date",
        FieldType::Unsupported(what) => what,
    }
}

fn bad_request(failure: Failure) -> ShardError {
    ShardError {
        status: 400,
        failure,
    }
}

/// `TextFieldMapper.TextFieldType.fielddataBuilder`'s refusal.
fn text_fielddata(name: &str) -> ShardError {
    bad_request(Failure::plain(
        "illegal_argument_exception",
        // ALLOC: error path; one message.
        format!(
            "Text fields are not optimised for operations that require per-document field data \
             like aggregations and sorting, so these operations are disabled by default. Please \
             use a keyword field instead. Alternatively, set fielddata=true on [{name}] in order \
             to load field data by uninverting the inverted index. Note that this can use \
             significant memory."
        ),
    ))
}

/// Resolves `agg`'s field in `shard`'s mapping.
fn resolve<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
) -> Result<Resolved, ShardError> {
    let Some(field) = shard.meta.mapping.get(&agg.field) else {
        return Ok(Resolved::Unmapped);
    };
    let format = match &field.ty {
        FieldType::Int { .. } | FieldType::Long => Format::Raw,
        FieldType::Date => Format::Date,
        FieldType::Text => return Err(text_fielddata(&agg.field)),
        FieldType::Keyword if agg.metric == Metric::Terms => {
            if agg.terms.as_ref().is_some_and(|t| t.min_doc_count == 0) {
                return Err(bad_request(Failure::plain(
                    "illegal_argument_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "[min_doc_count: 0] of the keyword field [{}] is not supported by this \
                         node yet",
                        agg.field
                    ),
                )));
            }
            Format::Raw
        }
        FieldType::Keyword if matches!(agg.metric, Metric::ValueCount | Metric::Cardinality) => {
            return Err(bad_request(Failure::plain(
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!(
                    "[{}] of the keyword field [{}] is not supported by this node yet",
                    agg.metric.name(),
                    agg.field
                ),
            )));
        }
        FieldType::Keyword => {
            return Err(bad_request(Failure::plain(
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!(
                    "Field [{}] of type [{}] is not supported for aggregation [{}]",
                    agg.field,
                    type_name(&field.ty),
                    agg.metric.name()
                ),
            )));
        }
        FieldType::Unsupported(what) => {
            return Err(bad_request(Failure::plain(
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!(
                    "aggregating {what} [{}] is not supported by this node yet",
                    agg.field
                ),
            )));
        }
    };
    if !field.doc_values {
        return Err(bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "Can't load fielddata on [{}] because fielddata is unsupported on fields of type \
                 [{}]. Use doc values instead.",
                agg.field,
                type_name(&field.ty)
            ),
        )));
    }
    if field.ty == FieldType::Keyword {
        return Ok(Resolved::Keyword);
    }
    Ok(Resolved::Numeric(format))
}

/// `agg`'s `terms` options (every `terms` has them).
fn terms_spec(agg: &MetricAgg) -> TermsSpec {
    agg.terms.as_deref().cloned().unwrap_or_default()
}

/// `agg`'s `histogram` options (every `histogram` has them).
fn histogram_spec(agg: &MetricAgg) -> HistogramSpec {
    agg.histogram.as_deref().cloned().unwrap_or_default()
}

/// What `avg` reads: the sum and the count.
const AVG_NEEDS: u8 = NEED_SUM | NEED_COUNT;

/// What `stats` reads: the count, the sum, the smallest and the largest.
const STATS_NEEDS: u8 = NEED_COUNT | NEED_SUM | NEED_MIN_MAX;

/// What a metric reads of its field's state.
fn needs(metric: Metric) -> u8 {
    match metric {
        Metric::Min => NEED_MIN,
        Metric::Max => NEED_MAX,
        Metric::Sum => NEED_SUM,
        Metric::Avg => AVG_NEEDS,
        Metric::Stats => STATS_NEEDS,
        Metric::ValueCount
        | Metric::Cardinality
        | Metric::Percentiles
        | Metric::PercentileRanks
        | Metric::TopHits
        | Metric::Terms
        | Metric::Histogram => NEED_COUNT,
    }
}

/// The shard-level result of `agg` from its field's `state`.
fn value_of(agg: &MetricAgg, state: &MetricState) -> Value {
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
        Metric::ValueCount => Value::ValueCount(state.count),
        Metric::Cardinality => Value::Cardinality(None),
        Metric::Percentiles | Metric::PercentileRanks => percentiles_of(agg, None),
        // ALLOC: one box a `terms`, bounded by the request's.
        Metric::Terms => Value::Terms(Box::new(terms_collect::empty(&terms_spec(agg)))),
        // ALLOC: one box a `histogram`, bounded by the request's.
        Metric::Histogram => {
            Value::Histogram(Box::new(histogram_collect::empty(&histogram_spec(agg))))
        }
        // Filled in by the search once the hits are collected.
        // ALLOC: one box a `top_hits`, bounded by the request's.
        Metric::TopHits => Value::TopHits(Box::new(TopHitsResult::empty())),
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
fn node(agg: &MetricAgg, precision: u32) -> AggNode {
    // ALLOC: one field name an aggregation, bounded by the request's.
    let field = agg.field.clone();
    // `byte` to `long` and `date` are longs read as they are.
    match agg.metric {
        Metric::Terms => terms_collect::node(&agg.field, &terms_spec(agg)),
        Metric::Histogram => histogram_collect::node(&agg.field, &histogram_spec(agg)),
        Metric::Cardinality => AggNode::Cardinality {
            field,
            kind: CardinalityKind::Numeric(ValueKind::Long),
            precision: Some(precision),
        },
        Metric::Percentiles | Metric::PercentileRanks => AggNode::Percentiles {
            field,
            missing: None,
            kind: ValueKind::Long,
            compression: agg.percentiles.compression,
            hdr: false,
        },
        metric => AggNode::Metric {
            field,
            missing: None,
            kind: ValueKind::Long,
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
fn value_from(agg: &MetricAgg, result: Option<AggResult>) -> Result<Value, ShardError> {
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
        // A long or a date is never NaN, which a digest refuses.
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
            Ok(Value::Terms(Box::new(terms_collect::from_engine(
                &terms_spec(agg),
                other,
                kept,
            ))))
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
        _ => Err(engine_error(&"an aggregation result of another kind")),
    }
}

/// The shard-level result of a keyword `terms` from the engine's `result`.
fn keyword_value(agg: &MetricAgg, result: Option<AggResult>) -> Result<Value, ShardError> {
    match result {
        Some(AggResult::Terms { buckets, .. }) => {
            let (other, kept) = buckets.into_iter().next().unwrap_or_default();
            // ALLOC: the kept buckets of one `terms`, at most its shard size.
            Ok(Value::KeywordTerms(Box::new(keyword_collect::from_engine(
                &terms_spec(agg),
                other,
                kept,
            ))))
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
        resolved.push(resolve(agg, shard)?);
        if agg.metric == Metric::Cardinality {
            check_execution_hint(agg)?;
        }
    }
    // ALLOC: as above.
    let mut nodes = Vec::with_capacity(aggs.len());
    for (agg, r) in aggs.iter().zip(&resolved) {
        let precision = match agg.metric {
            Metric::Cardinality => agg
                .cardinality
                .precision()
                .map_err(|m| bad_request(Failure::plain("illegal_argument_exception", m)))?,
            _ => DEFAULT_PRECISION,
        };
        match r {
            Resolved::Numeric(_) => nodes.push(node(agg, precision)),
            Resolved::Keyword => nodes.push(keyword_collect::node(&agg.field, &terms_spec(agg))),
            Resolved::Unmapped => {}
        }
    }
    let mut results = if nodes.is_empty() {
        // ALLOC: empty.
        Vec::new()
    } else {
        // ALLOC: one entry a keyword `terms`, bounded by the request's.
        let mut ords = HashMap::new();
        for (agg, r) in aggs.iter().zip(&resolved) {
            if matches!(r, Resolved::Keyword) && !ords.contains_key(&agg.field) {
                let built = shard
                    .reader
                    .global_ords(&agg.field)
                    .map_err(|e| engine_error(&e))?;
                // ALLOC: one field name a keyword field, as above.
                ords.insert(agg.field.clone(), built);
            }
        }
        // ALLOC: one index a segment: one slice, the whole shard in order.
        let slices = [(0..segments.len()).collect::<Vec<usize>>()];
        aggregate_tree(
            segments,
            shard.reader.segment_readers(),
            query,
            &nodes,
            &Globals::new(&ords).with_bucket_limit(BUCKET_LIMIT),
            &slices,
            None,
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
    for (agg, r) in aggs.iter().zip(resolved) {
        let (format, value) = match r {
            Resolved::Unmapped => (Format::Raw, value_of(agg, &MetricState::default())),
            Resolved::Numeric(format) => (format, value_from(agg, results.next())?),
            Resolved::Keyword => (Format::Raw, keyword_value(agg, results.next())?),
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
