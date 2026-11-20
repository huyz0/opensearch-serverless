//! Bucket aggregations on one shard. `terms` over a numeric field is served
//! ([`terms`]), over a keyword field ([`keyword_terms`]), and `histogram` over a numeric one ([`histogram`]); a bucket aggregation is held in the metric aggregations'
//! request and result types (`MetricAgg` and `Value`), as `top_hits` is, so
//! one pass, one response writer and one reduce serve them all.

pub mod count_order;
pub mod date_histogram;
pub mod date_histogram_collect;
pub mod date_histogram_parse;
pub mod date_histogram_render;
pub mod filters;
pub mod filters_collect;
pub mod filters_parse;
pub mod filters_render;
pub mod histogram;
pub mod histogram_collect;
pub mod histogram_parse;
pub mod histogram_render;
pub mod keyword_collect;
pub mod keyword_options;
pub mod keyword_subs;
pub mod keyword_terms;
pub mod parse_arms;
pub mod range;
pub mod range_collect;
pub mod range_parse;
pub mod range_render;
pub mod terms;
pub mod terms_collect;
pub mod terms_parse;
pub mod terms_render;

/// `search.max_buckets`'s default: the most distinct values one shard holds
/// for a `terms` before it refuses, which bounds the memory a high
/// cardinality field takes.
pub const BUCKET_LIMIT: usize = 65_535;

/// The shard's refusal of an aggregation with more buckets than the bucket
/// limit, for the types whose count the request sets.
pub fn check_limits(agg: &crate::aggs::metric::MetricAgg) -> Result<(), crate::search::ShardError> {
    range_collect::check_limit(agg)?;
    filters_collect::check_limit(agg)
}

/// The shard-level result of `agg`, a `range` or a `filters`, from the
/// engine's fixed buckets `docs`.
pub fn fixed_value(
    agg: &crate::aggs::metric::MetricAgg,
    docs: &[u64],
) -> crate::aggs::partial::Value {
    if agg.filters.is_some() {
        filters_collect::value(agg, docs)
    } else {
        range_collect::value(agg, docs)
    }
}

#[cfg(test)]
mod collect_tests;
#[cfg(test)]
mod date_histogram_collect_tests;
#[cfg(test)]
mod date_histogram_tests;
#[cfg(test)]
mod filters_collect_tests;
#[cfg(test)]
mod filters_tests;
#[cfg(test)]
mod histogram_collect_tests;
#[cfg(test)]
mod histogram_tests;
#[cfg(test)]
mod keyword_collect_tests;
#[cfg(test)]
mod keyword_options_tests;
#[cfg(test)]
mod keyword_subs_tests;
#[cfg(test)]
mod keyword_terms_tests;
#[cfg(test)]
mod range_collect_tests;
#[cfg(test)]
mod range_tests;
#[cfg(test)]
mod terms_tests;
