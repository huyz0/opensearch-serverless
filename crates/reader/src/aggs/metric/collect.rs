//! Running a request's metric aggregations on one shard.
//!
//! Each aggregation's field is resolved as `ValuesSourceConfig.resolve` and
//! `ValuesSourceRegistry.getAggregator` resolve it
//! (server/src/main/java/org/opensearch/search/aggregations/support/), at
//! OpenSearch 3.10.0: an unmapped field aggregates nothing, a numeric or
//! date field is read from its doc values, and any other type fails the
//! shard as OpenSearch fails it. The values come from one pass of
//! lucene-aggs' `metric_states` over the query's live matches, segment by
//! segment in document order: OpenSearch's aggregators ported, sums
//! compensated in the same order, so each state is the one OpenSearch's
//! aggregator builds on a shard searched without concurrent segment slices.

use lucene_aggs::metric_aggs::{
    MetricSpec, MetricState, NEED_COUNT, NEED_MAX, NEED_MIN, NEED_MIN_MAX, NEED_SUM, Source,
    ValueKind, metric_states,
};
use lucene_search::BooleanQuery;
use lucene_search::multi_segment::OpenSegment;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::metric::partial::{Format, Partial, Value};
use crate::aggs::metric::{Metric, MetricAgg};
use crate::failure::Failure;
use crate::mapping::FieldType;
use crate::search::ShardError;

/// What `ValuesSourceConfig` resolved an aggregation's field to.
enum Resolved {
    /// No such field: the aggregation sees no value.
    Unmapped,
    /// A numeric field's doc values, and how its values are shown.
    Numeric(Format),
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
        FieldType::Keyword if agg.metric == Metric::ValueCount => {
            return Err(bad_request(Failure::plain(
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!(
                    "[value_count] of the keyword field [{}] is not supported by this node yet",
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
    Ok(Resolved::Numeric(format))
}

/// What a metric reads of its field's state.
fn needs(metric: Metric) -> u8 {
    match metric {
        Metric::Min => NEED_MIN,
        Metric::Max => NEED_MAX,
        Metric::Sum => NEED_SUM,
        Metric::Avg => NEED_SUM | NEED_COUNT,
        Metric::Stats => NEED_COUNT | NEED_SUM | NEED_MIN_MAX,
        Metric::ValueCount => NEED_COUNT,
    }
}

/// The shard-level result of `metric` from its field's `state`.
fn value_of(metric: Metric, state: &MetricState) -> Value {
    match metric {
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
    }
}

/// Runs `aggs` over `query`'s live matches in `segments` of `shard`: one
/// result each, in request order. A field the aggregation cannot read
/// fails the shard with OpenSearch's status and exception.
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
    // ALLOC: as above.
    let mut specs = Vec::with_capacity(aggs.len());
    for agg in aggs {
        let r = resolve(agg, shard)?;
        if matches!(r, Resolved::Numeric(_)) {
            specs.push(MetricSpec {
                // ALLOC: one field name an aggregation, as above.
                field: agg.field.clone(),
                // `byte` to `long` and `date` are longs read as they are.
                kind: ValueKind::Long,
                source: Source::DocValues,
                needs: needs(agg.metric),
            });
        }
        resolved.push(r);
    }
    let states = if specs.is_empty() {
        // ALLOC: empty.
        Vec::new()
    } else {
        metric_states(segments, shard.reader.segment_readers(), query, &specs).map_err(|e| {
            ShardError {
                status: 500,
                // ALLOC: error path; one message.
                failure: Failure::opensearch("exception", e.to_string()),
            }
        })?
    };
    let mut states = states.iter();
    let empty = MetricState::default();
    let parts = aggs
        .iter()
        .zip(resolved)
        .map(|(agg, r)| {
            let (format, state) = match r {
                Resolved::Unmapped => (Format::Raw, &empty),
                Resolved::Numeric(format) => (format, states.next().unwrap_or(&empty)),
            };
            Partial {
                // ALLOC: one name an aggregation, as above.
                name: agg.name.clone(),
                format,
                value: value_of(agg.metric, state),
            }
        })
        // ALLOC: one result an aggregation, as above.
        .collect();
    Ok(parts)
}

#[cfg(test)]
#[path = "collect_tests.rs"]
mod tests;
