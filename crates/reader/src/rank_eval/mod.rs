//! `_rank_eval`: the request body and the metrics.
//!
//! The body parses from a recorded token stream as OpenSearch's `RankEvalSpec`
//! does; each rated request is a search whose hits a [`Metric`] scores against
//! the request's ratings.

pub mod metric;
mod metric_spec;
pub mod order;
pub mod spec;

pub use metric::{DocKey, Metric, Quality, combine};
pub use spec::{RankEvalSpec, RatedRequest, TemplateText};

#[cfg(test)]
mod tests;
