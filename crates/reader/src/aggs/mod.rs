//! Aggregations on one shard: each request's aggregations are parsed with
//! the search body, run over the query's matches beside the hits, and
//! answered as shard-level results a coordinator can reduce. Metric
//! aggregations are in [`metric`].

pub mod metric;
