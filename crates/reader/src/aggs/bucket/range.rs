//! The `range` aggregation's request and shard-level result
//! (`RangeAggregationBuilder`, `InternalRange` at OpenSearch 3.10.0), and
//! the reduce of results across shards (`InternalRange.reduce`).

use crate::aggs::bucket::running;
use crate::aggs::bucket::sub_values::reduce_fixed;
use crate::aggs::partial::{Format, Value};
use crate::date_pattern::DatePattern;

/// One range asked for: `from <= value < to`, an unbounded end an infinity
/// (`RangeAggregator.Range`).
#[derive(Clone, Debug, PartialEq)]
pub struct RangeDef {
    /// The `key` asked for; `None` generates it from the bounds.
    pub key: Option<String>,
    /// The inclusive lower bound; negative infinity for none.
    pub from: f64,
    /// The exclusive upper bound; positive infinity for none.
    pub to: f64,
}

/// What a `range` aggregation asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RangeSpec {
    /// The buckets are written as an object keyed by their key.
    pub keyed: bool,
    /// A `date_range`'s `format`, which prints its bounds and keys.
    pub format: Option<DatePattern>,
    /// The ranges, sorted by `from` and then `to` as
    /// `AbstractRangeBuilder.sortRanges` sorts them, equal ones in request
    /// order.
    pub ranges: Vec<RangeDef>,
}

/// One bucket of a `range`: `InternalRange.Bucket`.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeBucket {
    /// The `key` asked for; `None` generates it from the bounds.
    pub key: Option<String>,
    /// The inclusive lower bound.
    pub from: f64,
    /// The exclusive upper bound.
    pub to: f64,
    /// The documents in the range.
    pub doc_count: u64,
}

/// What one shard (or one reduce) holds for a `range`: `InternalRange`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RangeResult {
    /// Written as an object keyed by the buckets' keys.
    pub keyed: bool,
    /// The `format` that prints a date bound; `None` for the default.
    pub format: Option<DatePattern>,
    /// One bucket a range of the request, in the sorted order.
    pub buckets: Vec<RangeBucket>,
    /// The name and format of each sub-aggregation, in request order.
    pub sub_meta: Vec<(String, Format)>,
    /// Each bucket's sub-aggregation values, one list a bucket in the order
    /// of `buckets`; empty when there are no sub-aggregations.
    pub subs: Vec<Vec<Value>>,
}

/// Whether `a` and `b` answer the same request: the same `keyed` and, one
/// for one, the same key and bounds.
fn same_ranges(a: &RangeResult, b: &RangeResult) -> bool {
    a.keyed == b.keyed
        && a.sub_meta.len() == b.sub_meta.len()
        && a.buckets.len() == b.buckets.len()
        && a.buckets.iter().zip(&b.buckets).all(|(x, y)| {
            x.key == y.key && x.from.total_cmp(&y.from).is_eq() && x.to.total_cmp(&y.to).is_eq()
        })
}

/// `InternalRange.reduce`: each range's counts add. `None` unless there is
/// a part, every part is a `range` and all have the same ranges.
pub fn reduce(parts: &[Value]) -> Option<Value> {
    // ALLOC: one reference a shard, bounded by the shards that answered.
    let results: Option<Vec<&RangeResult>> = parts
        .iter()
        .map(|p| match p {
            Value::Range(r) => Some(r.as_ref()),
            _ => None,
        })
        // ALLOC: one reference a shard, as above.
        .collect();
    let results = results?;
    let (first, rest) = results.split_first()?;
    if rest.iter().any(|r| !same_ranges(first, r)) {
        return None;
    }
    let mut merged = (*first).clone();
    for other in rest {
        for (into, from) in merged.buckets.iter_mut().zip(&other.buckets) {
            into.doc_count = into.doc_count.saturating_add(from.doc_count);
        }
    }
    merged.subs = reduce_fixed(
        // ALLOC: one reference a shard, as above.
        &results
            .iter()
            .map(|r| r.subs.as_slice())
            .collect::<Vec<_>>(),
        merged.buckets.len(),
    );
    if !running::add(merged.buckets.len()) {
        return None;
    }
    // ALLOC: one box a `range`, bounded by the request's.
    Some(Value::Range(Box::new(merged)))
}
