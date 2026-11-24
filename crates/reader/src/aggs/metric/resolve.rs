//! Resolving an aggregation's field as `ValuesSourceConfig.resolve` and
//! `ValuesSourceRegistry.getAggregator` resolve it, at OpenSearch 3.10.0.

use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::composite_collect;
use crate::aggs::metric::{Metric, MetricAgg};
use crate::aggs::partial::Format;
use crate::failure::Failure;
use crate::mapping::FieldType;
use crate::search::ShardError;

/// What `ValuesSourceConfig` resolved an aggregation's field to.
pub(crate) enum Resolved {
    /// No such field: the aggregation sees no value.
    Unmapped,
    /// A numeric field's doc values, and how its values are shown.
    Numeric(Format),
    /// A keyword field's global ordinals, for a `terms`.
    Keyword,
    /// No field: a `filters` reads queries.
    Queries,
    /// No field: a `top_hits` reads the matches of its bucket.
    Hits,
    /// A `composite`: its sources read their own fields; whether it runs.
    Composite(bool),
}

/// The text of `Field [<name>] of type [<type>]`'s type.
pub(crate) fn type_name(ty: &FieldType) -> &str {
    match ty {
        FieldType::Text => "text",
        FieldType::Keyword => "keyword",
        FieldType::Int { name, .. } => name,
        FieldType::Long => "long",
        FieldType::Date => "date",
        FieldType::Unsupported(what) => what,
    }
}

pub(crate) fn bad_request(failure: Failure) -> ShardError {
    ShardError {
        status: 400,
        failure,
    }
}

/// `TextFieldMapper.TextFieldType.fielddataBuilder`'s refusal.
pub(crate) fn text_fielddata(name: &str) -> ShardError {
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
pub(crate) fn resolve<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
) -> Result<Resolved, ShardError> {
    if agg.filters.is_some() {
        return Ok(Resolved::Queries);
    }
    if agg.top_hits.is_some() {
        return Ok(Resolved::Hits);
    }
    if agg.composite.is_some() {
        return Ok(Resolved::Composite(composite_collect::runs(agg, shard)?));
    }
    let Some(field) = shard.meta.mapping.get(&agg.field) else {
        return Ok(Resolved::Unmapped);
    };
    let format = match &field.ty {
        FieldType::Int { .. } | FieldType::Long => Format::Raw,
        FieldType::Date => Format::Date,
        FieldType::Text => return Err(text_fielddata(&agg.field)),
        FieldType::Keyword if matches!(agg.metric, Metric::Terms | Metric::Cardinality) => {
            Format::Raw
        }
        FieldType::Keyword if agg.metric == Metric::ValueCount => {
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
    if let Some(option) = agg.terms.as_ref().and_then(|t| t.keyword.first_set()) {
        return Err(bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "[{option}] of the numeric field [{}] is not supported by this node yet",
                agg.field
            ),
        )));
    }
    crate::aggs::bucket::keyword_subs::refuse_numeric_order(agg)?;
    Ok(Resolved::Numeric(format))
}
