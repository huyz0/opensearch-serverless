//! Pipeline aggregations at OpenSearch 3.10.0: the sibling ones (`avg_bucket`,
//! `sum_bucket`, `min_bucket`, `max_bucket` and `stats_bucket`) and the parent
//! ones inside a bucket aggregation (`derivative`, `cumulative_sum` and
//! `bucket_sort`). Parsed beside the aggregations a shard collects, checked as
//! `AggregatorFactories.Builder.validate` checks them, and computed after the
//! final reduce over the rendered buckets of the aggregation their
//! `buckets_path` names.

pub mod compute;
pub mod parent;
pub mod parent_parse;
pub mod parse;
mod sort_parse;

#[cfg(test)]
mod parent_parse_tests;
#[cfg(test)]
mod parent_tests;

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
    /// `derivative`, a parent pipeline.
    Derivative,
    /// `cumulative_sum`, a parent pipeline.
    CumulativeSum,
    /// `bucket_sort`, a parent pipeline.
    BucketSort,
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
            "derivative" => Self::Derivative,
            "cumulative_sum" => Self::CumulativeSum,
            "bucket_sort" => Self::BucketSort,
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
            Self::Derivative => "derivative",
            Self::CumulativeSum => "cumulative_sum",
            Self::BucketSort => "bucket_sort",
        }
    }

    /// Whether the pipeline sits inside the bucket aggregation it works on
    /// and changes its buckets, rather than beside it.
    pub fn is_parent(self) -> bool {
        matches!(
            self,
            Self::Derivative | Self::CumulativeSum | Self::BucketSort
        )
    }

    /// The type `typed_keys` writes before the result's name
    /// (`InternalAggregation.getWriteableName`).
    pub fn result_type(self) -> &'static str {
        match self {
            Self::Avg | Self::Sum => "simple_value",
            Self::Min | Self::Max => "bucket_metric_value",
            Self::Stats => "stats_bucket",
            Self::Derivative | Self::CumulativeSum => "simple_value",
            Self::BucketSort => "bucket_sort",
        }
    }
}

/// What a bucket contributes: its document count, or a metric of one of its
/// sub-aggregations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BucketValue {
    /// `_count`.
    Count,
    /// `_key`, which only a `bucket_sort` reads.
    Key,
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

/// One pipeline aggregation of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineAgg {
    /// The aggregation's name, its key in the response.
    pub name: String,
    /// What it computes.
    pub kind: PipelineKind,
    /// The path as written, for messages.
    pub path: String,
    /// The aggregation whose buckets it reads: the one beside a sibling
    /// pipeline, the one holding a parent pipeline.
    pub agg: String,
    /// What it reads from each bucket.
    pub value: BucketValue,
    /// `gap_policy` of `insert_zeros`: a bucket with no value counts as 0
    /// rather than being skipped.
    pub insert_zeros: bool,
    /// The bucket aggregation a parent pipeline sits in; `None` for a
    /// sibling.
    pub host: Option<String>,
    /// A `bucket_sort`'s sorts and window; `None` for every other pipeline.
    pub sort: Option<SortSpec>,
}

/// A `bucket_sort`'s sorts and window over its buckets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SortSpec {
    /// What each sort reads from a bucket, and whether it is descending.
    pub fields: Vec<(BucketValue, bool)>,
    /// The buckets skipped from the front.
    pub from: usize,
    /// The most buckets kept; all when `None`.
    pub size: Option<usize>,
}
