//! Sibling pipeline aggregations (`avg_bucket`, `sum_bucket`, `min_bucket`,
//! `max_bucket` and `stats_bucket`) at OpenSearch 3.10.0: parsed beside the
//! aggregations a shard collects, checked as `AggregatorFactories.Builder.validate`
//! checks them, and computed after the final reduce over the rendered buckets
//! of the aggregation their `buckets_path` names.

pub mod compute;
pub mod parse;

#[cfg(test)]
mod pipeline_tests;

/// What a sibling pipeline aggregation computes over its buckets' values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineKind {
    /// `avg_bucket`.
    Avg,
    /// `sum_bucket`.
    Sum,
    /// `min_bucket`.
    Min,
    /// `max_bucket`.
    Max,
    /// `stats_bucket`.
    Stats,
}

impl PipelineKind {
    /// The kind a type name in `aggs` names.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "avg_bucket" => Self::Avg,
            "sum_bucket" => Self::Sum,
            "min_bucket" => Self::Min,
            "max_bucket" => Self::Max,
            "stats_bucket" => Self::Stats,
            _ => return None,
        })
    }

    /// The name in a request.
    pub fn name(self) -> &'static str {
        match self {
            Self::Avg => "avg_bucket",
            Self::Sum => "sum_bucket",
            Self::Min => "min_bucket",
            Self::Max => "max_bucket",
            Self::Stats => "stats_bucket",
        }
    }

    /// The type `typed_keys` writes before the result's name
    /// (`InternalAggregation.getWriteableName`).
    pub fn result_type(self) -> &'static str {
        match self {
            Self::Avg | Self::Sum => "simple_value",
            Self::Min | Self::Max => "bucket_metric_value",
            Self::Stats => "stats_bucket",
        }
    }
}

/// What a bucket contributes: its document count, or a metric of one of its
/// sub-aggregations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BucketValue {
    /// `_count`.
    Count,
    /// `sub` or `sub.metric`: the value of the named member of the
    /// sub-aggregation's body (`value` when no metric is named).
    Sub {
        /// The sub-aggregation's name.
        name: String,
        /// The member of its body.
        metric: String,
    },
    /// A path this node reads no value from: the aggregation alone, or more
    /// than two elements. It validates as Java does, then is refused.
    Other {
        /// The element after the aggregation's, if any.
        second: Option<String>,
    },
}

/// One sibling pipeline aggregation of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineAgg {
    /// The aggregation's name, its key in the response.
    pub name: String,
    /// What it computes.
    pub kind: PipelineKind,
    /// The path as written, for messages.
    pub path: String,
    /// The aggregation whose buckets it reads.
    pub agg: String,
    /// What it reads from each bucket.
    pub value: BucketValue,
    /// `gap_policy` of `insert_zeros`: a bucket with no value counts as 0
    /// rather than being skipped.
    pub insert_zeros: bool,
}
