//! The shard-level result of one aggregation, and its reduce across shards.
//!
//! Each [`Value`] holds what OpenSearch's `Internal*` aggregation for it
//! holds, so shard results reduce to what OpenSearch's coordinator computes
//! from them. Metric values reduce in [`crate::aggs::metric::partial`],
//! bucket values in [`crate::aggs::bucket`]; [`reduce`] picks by kind.

use crate::aggs::bucket::keyword_terms::{self, KeywordTermsResult};
use crate::aggs::bucket::terms::{self, TermsResult};
use crate::aggs::metric::hll::Hll;
use crate::aggs::metric::partial::{reduce_cardinality, reduce_percentiles, reduce_scalar};
use crate::aggs::metric::tdigest::Digest;
use crate::aggs::metric::top_hits::TopHitsResult;

/// How a value is shown beside its number (`DocValueFormat`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `DocValueFormat.RAW`: the number alone.
    Raw,
    /// `DocValueFormat.DateTime` in the default date format: each value also
    /// as `strict_date_optional_time` text.
    Date,
}

/// What one shard computed for one aggregation.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `InternalMin.min`: positive infinity when there was no value.
    Min(f64),
    /// `InternalMax.max`: negative infinity when there was no value.
    Max(f64),
    /// `InternalSum.sum`: the compensated sum's value.
    Sum(f64),
    /// `InternalAvg`'s sum and count.
    Avg {
        /// The compensated sum's value.
        sum: f64,
        /// How many values.
        count: u64,
    },
    /// `InternalStats`.
    Stats {
        /// How many values.
        count: u64,
        /// The compensated sum's value.
        sum: f64,
        /// The smallest value; positive infinity for none.
        min: f64,
        /// The largest value; negative infinity for none.
        max: f64,
    },
    /// `InternalValueCount.value`.
    ValueCount(u64),
    /// `InternalCardinality.counts`: `None` when no value was seen.
    Cardinality(Option<Hll>),
    /// `AbstractInternalTDigestPercentiles`: the keys asked for, how they
    /// are written, and the digest, settled.
    Percentiles {
        /// `percentile_ranks` rather than `percentiles`.
        ranks: bool,
        /// The percents or the values ranked, sorted.
        keys: Vec<f64>,
        /// Written as an object rather than an array.
        keyed: bool,
        /// The compression asked for: what a reduce's digest is made with.
        compression: f64,
        /// `None` when no document matched.
        digest: Option<Digest>,
    },
    /// `InternalTopHits`: the shard's top hits, which a coordinator merges
    /// by their sort once it has fetched them.
    TopHits(Box<TopHitsResult>),
    /// `LongTerms`: a numeric `terms`' buckets.
    Terms(Box<TermsResult>),
    /// `StringTerms`: a keyword `terms`' buckets.
    KeywordTerms(Box<KeywordTermsResult>),
}

/// One aggregation's shard-level result.
#[derive(Clone, Debug, PartialEq)]
pub struct Partial {
    /// The aggregation's name.
    pub name: String,
    /// The format of its field.
    pub format: Format,
    /// What was computed.
    pub value: Value,
}

/// The reduce of the shard results `parts`, all of one aggregation: `None`
/// when there are none, they are not all the same kind, or two sketches'
/// precisions differ (which OpenSearch refuses).
pub fn reduce(parts: &[Value]) -> Option<Value> {
    match parts.first()? {
        Value::Cardinality(_) => reduce_cardinality(parts),
        Value::Percentiles { .. } => reduce_percentiles(parts),
        Value::Terms(_) => terms::reduce(parts),
        Value::KeywordTerms(_) => keyword_terms::reduce(parts),
        // A `top_hits` has no reduce here: merging hits needs them fetched
        // and their shard known, so `reduce_scalar` answers `None` for it.
        _ => reduce_scalar(parts),
    }
}
