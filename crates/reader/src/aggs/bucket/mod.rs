//! Bucket aggregations on one shard. `terms` over a numeric field is served
//! ([`terms`]), and over a keyword field ([`keyword_terms`]); a bucket aggregation is held in the metric aggregations'
//! request and result types (`MetricAgg` and `Value`), as `top_hits` is, so
//! one pass, one response writer and one reduce serve them all.

pub mod keyword_collect;
pub mod keyword_options;
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
mod keyword_collect_tests;
#[cfg(test)]
mod keyword_options_tests;
#[cfg(test)]
mod keyword_terms_tests;
#[cfg(test)]
mod terms_tests;
