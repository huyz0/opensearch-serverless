//! Bucket aggregations on one shard. `terms` over a numeric field is served
//! ([`terms`]), over a keyword field ([`keyword_terms`]), and `histogram` over a numeric one ([`histogram`]); a bucket aggregation is held in the metric aggregations'
//! request and result types (`MetricAgg` and `Value`), as `top_hits` is, so
//! one pass, one response writer and one reduce serve them all.

pub mod composite;
pub mod composite_collect;
pub mod composite_parse;
pub mod composite_render;
pub mod count_order;
pub mod date_histogram;
pub mod date_histogram_collect;
pub mod date_histogram_parse;
pub mod date_histogram_render;
pub mod date_histogram_zone;
pub mod filters;
pub mod filters_collect;
pub mod filters_parse;
pub mod filters_render;
pub mod fixed_subs;
pub mod histogram;
pub mod histogram_collect;
pub mod histogram_parse;
pub mod histogram_render;
pub mod histogram_sub_order;
pub mod keyed_subs;
pub mod keyword_collect;
pub mod keyword_options;
pub mod keyword_subs;
pub mod keyword_terms;
pub mod missing;
pub mod nested_agg;
pub mod nested_subs;
pub mod parse_arms;
pub mod range;
pub mod range_collect;
pub mod range_parse;
pub mod range_render;
pub mod rare;
pub mod rare_collect;
pub mod rare_parse;
pub mod running;
pub mod significant;
pub mod significant_collect;
pub mod significant_parse;
pub mod sub_order;
pub mod sub_values;
pub mod terms;
pub mod terms_collect;
pub mod terms_parse;
pub mod terms_render;
pub mod terms_sub_order;
pub mod walk;
pub mod with_subs;

/// `search.max_buckets`'s default: the most distinct values one shard holds
/// for a `terms` before it refuses, which bounds the memory a high
/// cardinality field takes.
pub const BUCKET_LIMIT: usize = 65_535;

/// The shard's refusal of an aggregation with more buckets than the bucket
/// limit, for the types whose count the request sets.
pub fn check_limits(agg: &crate::aggs::metric::MetricAgg) -> Result<(), crate::search::ShardError> {
    range_collect::check_limit(agg)?;
    filters_collect::check_limit(agg)?;
    composite_collect::check_limit(agg)
}

use crate::aggs::metric::MetricAgg;
use date_histogram::DateHistogramSpec;
use histogram::HistogramSpec;
use terms::TermsSpec;

/// `agg`'s `terms` options (every `terms` has them).
pub(crate) fn terms_spec(agg: &MetricAgg) -> TermsSpec {
    agg.terms.as_deref().cloned().unwrap_or_default()
}

/// `agg`'s `histogram` options (every `histogram` has them).
pub(crate) fn histogram_spec(agg: &MetricAgg) -> HistogramSpec {
    agg.histogram.as_deref().cloned().unwrap_or_default()
}

/// `agg`'s `date_histogram` options (every `date_histogram` has them).
pub(crate) fn date_histogram_spec(agg: &MetricAgg) -> DateHistogramSpec {
    agg.date_histogram.as_deref().cloned().unwrap_or_default()
}

#[cfg(test)]
mod collect_tests;
#[cfg(test)]
mod composite_collect_tests;
#[cfg(test)]
mod composite_tests;
#[cfg(test)]
mod date_histogram_collect_tests;
#[cfg(test)]
mod date_histogram_format_tests;
#[cfg(test)]
mod date_histogram_region_tests;
#[cfg(test)]
mod date_histogram_tests;
#[cfg(test)]
mod date_range_tests;
#[cfg(test)]
mod filters_collect_tests;
#[cfg(test)]
mod filters_tests;
#[cfg(test)]
mod fixed_subs_tests;
#[cfg(test)]
mod global_tests;
#[cfg(test)]
mod histogram_collect_tests;
#[cfg(test)]
mod histogram_sub_order_tests;
#[cfg(test)]
mod histogram_tests;
#[cfg(test)]
mod keyed_subs_tests;
#[cfg(test)]
mod keyword_collect_tests;
#[cfg(test)]
mod keyword_options_tests;
#[cfg(test)]
mod keyword_order_tests;
#[cfg(test)]
mod keyword_partition_tests;
#[cfg(test)]
mod keyword_subs_tests;
#[cfg(test)]
mod keyword_terms_tests;

#[cfg(test)]
mod missing_collect_tests;
#[cfg(test)]
mod missing_tests;
#[cfg(test)]
mod nested_agg_collect_tests;
#[cfg(test)]
mod nested_agg_tests;
#[cfg(test)]
mod nested_filters_tests;
#[cfg(test)]
mod nested_soak_tests;
#[cfg(test)]
mod nested_terms_tests;
#[cfg(test)]
mod nested_tests;
#[cfg(test)]
mod nested_under_terms_tests;
#[cfg(test)]
mod range_collect_tests;
#[cfg(test)]
mod range_tests;
#[cfg(test)]
mod rare_tests;
#[cfg(test)]
mod running_tests;
#[cfg(test)]
mod significant_tests;
#[cfg(test)]
mod sub_order_tests;
#[cfg(test)]
mod terms_error_tests;
#[cfg(test)]
mod terms_subs_tests;
#[cfg(test)]
mod terms_tests;
#[cfg(test)]
mod top_hits_sub_tests;
