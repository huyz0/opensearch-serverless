//! Bucket aggregations on one shard. `terms` over a numeric field is served
//! ([`terms`]); a bucket aggregation is held in the metric aggregations'
//! request and result types (`MetricAgg` and `Value`), as `top_hits` is, so
//! one pass, one response writer and one reduce serve them all.

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
mod terms_tests;
