//! Bucket aggregations on one shard. `terms` over a numeric field is served
//! ([`terms`]), over a keyword field ([`keyword_terms`]), and `histogram` over a numeric one ([`histogram`]); a bucket aggregation is held in the metric aggregations'
//! request and result types (`MetricAgg` and `Value`), as `top_hits` is, so
//! one pass, one response writer and one reduce serve them all.

pub mod count_order;
pub mod date_histogram;
pub mod date_histogram_collect;
pub mod date_histogram_parse;
pub mod date_histogram_render;
pub mod histogram;
pub mod histogram_collect;
pub mod histogram_parse;
pub mod histogram_render;
pub mod keyword_collect;
pub mod keyword_options;
pub mod keyword_subs;
pub mod keyword_terms;
pub mod terms;
pub mod terms_collect;
pub mod terms_parse;
pub mod terms_render;

/// `search.max_buckets`'s default: the most distinct values one shard holds
/// for a `terms` before it refuses, which bounds the memory a high
/// cardinality field takes.
pub const BUCKET_LIMIT: usize = 65_535;

#[cfg(test)]
mod collect_tests;
#[cfg(test)]
mod date_histogram_collect_tests;
#[cfg(test)]
mod date_histogram_tests;
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
mod terms_tests;
