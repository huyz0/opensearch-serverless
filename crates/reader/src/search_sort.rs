//! The sort of a search: `SortBuilder.buildSort` and `FieldSortBuilder.build`
//! at OpenSearch 3.10.0, turning the body's sort keys into the engine's.

use std::sync::Arc;

use lucene_search::top_field::{HalfFloatSort, ScaledFloatSort, Selector, SortField, SortType};
use lucene_util::numeric_utils::{double_to_sortable_long, float_to_sortable_int};
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::dsl::SortKey;
use crate::failure::{Failure, Meta};
use crate::lucene_query::QueryContext;
use crate::mapping::FieldType;
use crate::search::ShardError;
use crate::search_sort_nested::nested_field_sort;

/// How a key orders, and how its values read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyKind {
    Score,
    Numeric,
    /// A `double` field: a sortable long.
    Double,
    /// A `float` field: a sortable int.
    Float,
    Term,
    /// A script's number, held as a sortable long.
    Script,
}

/// Whether a sort reads a numeric field, which needs the points open.
pub(crate) fn reads_points(kinds: &[KeyKind]) -> bool {
    kinds
        .iter()
        .any(|k| matches!(k, KeyKind::Numeric | KeyKind::Double | KeyKind::Float))
}

pub(crate) type Sort = Option<(Vec<SortField>, Vec<KeyKind>)>;

/// `SortBuilder.buildSort`: `None` for a sort by score alone.
pub(crate) fn sort_fields<S: BlobStore + 'static>(
    keys: &[SortKey],
    shard: &OpenShard<S>,
    index: &str,
    cx: Option<&QueryContext<'_>>,
) -> Result<Sort, ShardError> {
    if keys.is_empty() || keys == [SortKey::Score { ascending: false }] {
        return Ok(None);
    }
    // ALLOC: one entry a sort key, at most MAX_SORT_KEYS.
    let mut fields = Vec::with_capacity(keys.len());
    // ALLOC: one entry a sort key, at most MAX_SORT_KEYS.
    let mut kinds = Vec::with_capacity(keys.len());
    for key in keys {
        let (field, kind) = match key {
            SortKey::Score { ascending } => {
                let mut f = SortField::score();
                f.reverse = *ascending;
                (f, KeyKind::Score)
            }
            SortKey::Field {
                field,
                ascending,
                nested: None,
            } => field_sort(field, !*ascending, shard, index)?,
            SortKey::Field {
                field,
                ascending,
                nested: Some(nested),
            } => nested_field_sort(field, !*ascending, nested, shard, index, cx)?,
            SortKey::GeoDistance(sort) => crate::search_sort_geo::geo_sort(sort, shard)?,
            SortKey::Script { script, ascending } => {
                let Some(cx) = cx else {
                    return Err(ShardError::bad_request(Failure::plain(
                        "illegal_argument_exception",
                        "a [_script] sort is not supported by this node yet here",
                    )));
                };
                let source = crate::dsl::queries::function_score_script::sort(cx, script)
                    .map_err(ShardError::bad_request)?;
                (
                    SortField::with_source("_script", source, !*ascending),
                    KeyKind::Script,
                )
            }
        };
        fields.push(field);
        kinds.push(kind);
    }
    Ok(Some((fields, kinds)))
}

/// `FieldSortBuilder.build`: missing values last, whichever the order.
pub(crate) fn field_sort<S: BlobStore + 'static>(
    name: &str,
    reverse: bool,
    shard: &OpenShard<S>,
    index: &str,
) -> Result<(SortField, KeyKind), ShardError> {
    if name == "_doc" {
        let mut f = SortField::doc();
        f.reverse = reverse;
        return Ok((f, KeyKind::Numeric));
    }
    if name == "_seq_no" {
        let mut f = SortField::numeric(name, SortType::Long, reverse);
        f.missing = if reverse { i64::MIN } else { i64::MAX };
        return Ok((f, KeyKind::Numeric));
    }
    let Some(field) = shard.meta.mapping.get(name) else {
        return Err(ShardError::bad_request(
            Failure::opensearch(
                "query_shard_exception",
                // ALLOC: error path; one message.
                format!("No mapping found for [{name}] in order to sort on"),
            )
            // ALLOC: error path; the index name.
            .with_meta("index", Meta::Str(index.to_owned()))
            // ALLOC: error path; the index uuid.
            .with_meta("index_uuid", Meta::Str(shard.meta.uuid.clone())),
        ));
    };
    let unsupported = |what: &str| {
        ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("sorting on {what} is not supported by this node yet"),
        ))
    };
    if matches!(field.ty, FieldType::Text) {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "Text fields are not optimised for operations that require per-document field \
                 data like aggregations and sorting, so these operations are disabled by default. \
                 Please use a keyword field instead. Alternatively, set fielddata=true on [{name}] \
                 in order to load field data by uninverting the inverted index. Note that this can \
                 use significant memory."
            ),
        )));
    }
    if !field.doc_values {
        // ALLOC: error path; one message.
        return Err(unsupported(&format!(
            "the field [{name}] without doc values"
        )));
    }
    let (ty, min, max) = match &field.ty {
        FieldType::Keyword => {
            let mut f = SortField::string(name, reverse);
            f.missing = i64::from(!reverse);
            return Ok((f, KeyKind::Term));
        }
        FieldType::Int { .. } => (SortType::Int, i64::from(i32::MIN), i64::from(i32::MAX)),
        FieldType::Long | FieldType::Date => (SortType::Long, i64::MIN, i64::MAX),
        // ALLOC: error path; one message.
        FieldType::Double => {
            let mut f = SortField::numeric(name, SortType::Double, reverse);
            f.missing = double_to_sortable_long(if reverse {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            });
            return Ok((f, KeyKind::Double));
        }
        FieldType::Float => {
            let mut f = SortField::numeric(name, SortType::Float, reverse);
            f.missing = i64::from(float_to_sortable_int(if reverse {
                f32::NEG_INFINITY
            } else {
                f32::INFINITY
            }));
            return Ok((f, KeyKind::Float));
        }
        FieldType::Scaled(bits) => {
            // `ScaledFloatFieldMapper`'s comparator: the scaled values as doubles.
            let selector = if reverse {
                Selector::Max
            } else {
                Selector::Min
            };
            let missing = if reverse {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            };
            let inverse = 1.0 / f64::from_bits(*bits);
            let source = ScaledFloatSort {
                selector,
                missing,
                inverse,
            };
            return Ok((
                SortField::with_source(name, Arc::new(source), reverse),
                KeyKind::Double,
            ));
        }
        FieldType::HalfFloat => {
            // `HalfFloatValuesComparatorSource`: the default mode picks the smallest value
            // ascending and the largest descending, and a hit's key is a float.
            let selector = if reverse {
                Selector::Max
            } else {
                Selector::Min
            };
            let missing = if reverse {
                f32::NEG_INFINITY
            } else {
                f32::INFINITY
            };
            let f = SortField::with_source(
                name,
                Arc::new(HalfFloatSort { selector, missing }),
                reverse,
            );
            return Ok((f, KeyKind::Float));
        }
        // ALLOC: error path; one message.
        other @ (FieldType::Unsupported(_) | FieldType::GeoPoint) => {
            return Err(unsupported(&format!("{} [{name}]", other.refusal_name())));
        }
        FieldType::Text => return Err(unsupported("a text field")),
    };
    let mut f = SortField::numeric(name, ty, reverse);
    f.missing = if reverse { min } else { max };
    Ok((f, KeyKind::Numeric))
}

#[cfg(test)]
#[path = "search_sort_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "search_nested_sort_tests.rs"]
mod nested_tests;
