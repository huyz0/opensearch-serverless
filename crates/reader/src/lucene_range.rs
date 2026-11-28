//! A range query on a numeric or date field.
//!
//! Ported from `RangeQueryBuilder.doToQuery` and `DateFieldType.rangeQuery`
//! (server/src/main/java/org/opensearch/index/query/RangeQueryBuilder.java,
//! server/src/main/java/org/opensearch/index/mapper/DateFieldMapper.java) at OpenSearch 3.10.0.

use lucene_search::Clause;
use lucene_search::extended_query::PointRangeQuery;

use crate::date::DateSpec;
use crate::date_math;
use crate::dsl::Value;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, field_type, match_none, point, unsupported};
use crate::mapping::FieldType;
use crate::numbers;
use crate::numbers_decimal as decimal;

/// `DateFieldType.rangeQuery` over the default format.
pub(crate) fn date_range(
    cx: &QueryContext<'_>,
    field: &str,
    from: Option<&Value>,
    to: Option<&Value>,
    (include_lower, include_upper): (bool, bool),
    spec: DateSpec,
) -> Result<Clause, Failure> {
    let lower = match from {
        Some(v) => date_math::lower_bound_with(v.text(), include_lower, cx.now_millis, spec)
            .map_err(|e| e.into_failure_in(v.text(), spec.format))?,
        None => i64::MIN,
    };
    let upper = match to {
        Some(v) => date_math::upper_bound_with(v.text(), include_upper, cx.now_millis, spec)
            .map_err(|e| e.into_failure_in(v.text(), spec.format))?,
        None => i64::MAX,
    };
    if lower > upper {
        return Ok(match_none());
    }
    point(PointRangeQuery::long_range(field, &[lower], &[upper]))
}

/// `RangeQueryBuilder.doToQuery` for numeric and date fields.
pub(crate) fn range(
    cx: &QueryContext<'_>,
    field: &str,
    from: Option<&Value>,
    to: Option<&Value>,
    include_lower: bool,
    include_upper: bool,
) -> Result<Clause, Failure> {
    range_in(
        cx,
        field,
        from,
        to,
        (include_lower, include_upper),
        DateSpec::default(),
    )
}

/// [`range`] with the `format` and `time_zone` the query names, which only a date field takes.
pub(crate) fn range_in(
    cx: &QueryContext<'_>,
    field: &str,
    from: Option<&Value>,
    to: Option<&Value>,
    (include_lower, include_upper): (bool, bool),
    spec: DateSpec,
) -> Result<Clause, Failure> {
    let Some(ty) = field_type(cx, field)? else {
        return Ok(match_none());
    };
    if spec.named && *ty != FieldType::Date {
        // ALLOC: error path; one message.
        return Err(unsupported(&format!(
            "a [range] [format] or [time_zone] on the field [{field}], which is not a date"
        )));
    }
    match ty {
        FieldType::Int { .. } => {
            match numbers::int_range(from, to, include_lower, include_upper)? {
                Some((l, u)) => point(PointRangeQuery::int_range(field, &[l], &[u])),
                None => Ok(match_none()),
            }
        }
        FieldType::Long => match numbers::long_range(from, to, include_lower, include_upper)? {
            Some((l, u)) => point(PointRangeQuery::long_range(field, &[l], &[u])),
            None => Ok(match_none()),
        },
        FieldType::Date => date_range(cx, field, from, to, (include_lower, include_upper), spec),
        FieldType::Double => match decimal::double_range(from, to, include_lower, include_upper)? {
            Some((l, u)) => point(decimal::double_points(field, l, u)),
            None => Ok(match_none()),
        },
        FieldType::Float => match decimal::float_range(from, to, include_lower, include_upper)? {
            Some((l, u)) => point(decimal::float_points(field, l, u)),
            None => Ok(match_none()),
        },
        FieldType::Scaled(bits) => {
            let factor = f64::from_bits(*bits);
            match decimal::scaled_range(from, to, include_lower, include_upper, factor)? {
                Some((l, u)) => point(PointRangeQuery::long_range(field, &[l], &[u])),
                None => Ok(match_none()),
            }
        }
        FieldType::HalfFloat => {
            match decimal::half_range(from, to, include_lower, include_upper)? {
                Some((l, u)) => point(decimal::half_points(field, l, u)),
                None => Ok(match_none()),
            }
        }
        FieldType::Text | FieldType::Keyword | FieldType::GeoPoint | FieldType::Unsupported(_) => {
            // ALLOC: error path; one message.
            Err(unsupported(&format!(
                "a range query on the string field [{field}]"
            )))
        }
    }
}
