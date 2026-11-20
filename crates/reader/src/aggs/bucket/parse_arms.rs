//! The bucket aggregations' bodies, parsed into the `MetricAgg` that holds
//! them: each type's parser lives beside its other code.

use crate::aggs::bucket::date_histogram_parse::parse_date_histogram_body;
use crate::aggs::bucket::filters_parse::parse_filters_body;
use crate::aggs::bucket::histogram_parse::parse_histogram_body;
use crate::aggs::bucket::range_parse::parse_range_body;
use crate::aggs::bucket::terms_parse::parse_terms_body;
use crate::aggs::metric::{Metric, MetricAgg};
use crate::dsl::error::DslError;
use crate::dsl::tokens::Parser;

/// Parses the body of `agg` when it is a bucket aggregation, the parser on
/// the body's start, and answers whether it was: any other type is left to
/// the caller. Each spec is boxed, bounded by the body's recording.
pub fn parse_bucket(p: &mut Parser<'_>, agg: &mut MetricAgg) -> Result<bool, DslError> {
    let name = agg.name.clone();
    match agg.metric {
        Metric::Terms => {
            let (field, spec) = parse_terms_body(p, &name)?;
            // ALLOC: one box a `terms`, bounded by the body's recording.
            (agg.field, agg.terms) = (field, Some(Box::new(spec)));
        }
        Metric::Histogram => {
            let (field, spec) = parse_histogram_body(p, &name)?;
            // ALLOC: one box a `histogram`, bounded by the body's recording.
            (agg.field, agg.histogram) = (field, Some(Box::new(spec)));
        }
        Metric::DateHistogram => {
            let (field, spec) = parse_date_histogram_body(p, &name)?;
            // ALLOC: one box a `date_histogram`, bounded by the body's recording.
            (agg.field, agg.date_histogram) = (field, Some(Box::new(spec)));
        }
        Metric::Range => {
            let (field, spec) = parse_range_body(p, &name)?;
            // ALLOC: one box a `range`, bounded by the body's recording.
            (agg.field, agg.range) = (field, Some(Box::new(spec)));
        }
        Metric::Filters => {
            // ALLOC: one box a `filters`, bounded by the body's recording.
            agg.filters = Some(Box::new(parse_filters_body(p, &name)?));
        }
        _ => return Ok(false),
    }
    Ok(true)
}
