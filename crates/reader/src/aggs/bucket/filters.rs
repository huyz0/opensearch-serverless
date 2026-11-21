//! The `filters` aggregation's request and shard-level result
//! (`FiltersAggregationBuilder`, `InternalFilters` at OpenSearch 3.10.0),
//! and the reduce of results across shards (`InternalFilters.reduce`).

use std::cmp::Ordering;

use crate::aggs::bucket::sub_values::reduce_fixed;
use crate::aggs::partial::{Format, Value};
use crate::dsl::Query;

/// The key of the bucket for documents matching no filter, unless asked.
pub const DEFAULT_OTHER_KEY: &str = "_other_";

/// What a `filters` aggregation asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FiltersSpec {
    /// A `filter`: one filter, written as its document count alone.
    pub single: bool,
    /// The filters were given as an object: the buckets are written as an
    /// object named by their keys.
    pub keyed: bool,
    /// The key of the bucket of documents matching no filter; `None` for no
    /// such bucket.
    pub other_key: Option<String>,
    /// `(key, filter)`: sorted by key when keyed (as the builder sorts
    /// them), in request order, keyed `0`, `1`, ..., otherwise.
    pub filters: Vec<(String, Query)>,
}

/// `String.compareTo`: by UTF-16 code unit.
pub fn compare_keys(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// What one shard (or one reduce) holds for a `filters`: `InternalFilters`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FiltersResult {
    /// A `filter`'s result: one bucket, written as its document count alone.
    pub single: bool,
    /// Written as an object named by the keys.
    pub keyed: bool,
    /// `(key, doc count)`: one a filter, then the other bucket if asked.
    pub buckets: Vec<(String, u64)>,
    /// The name and format of each sub-aggregation, in request order.
    pub sub_meta: Vec<(String, Format)>,
    /// Each bucket's sub-aggregation values, one list a bucket in the order
    /// of `buckets`; empty when there are no sub-aggregations.
    pub subs: Vec<Vec<Value>>,
}

/// `InternalFilters.reduce`: each bucket's counts add. `None` unless there
/// is a part, every part is a `filters` and all have the same buckets.
pub fn reduce(parts: &[Value]) -> Option<Value> {
    // ALLOC: one reference a shard, bounded by the shards that answered.
    let results: Option<Vec<&FiltersResult>> = parts
        .iter()
        .map(|p| match p {
            Value::Filters(r) => Some(r.as_ref()),
            _ => None,
        })
        // ALLOC: one reference a shard, as above.
        .collect();
    let results = results?;
    let (first, rest) = results.split_first()?;
    let same = |r: &&FiltersResult| {
        r.keyed == first.keyed
            && r.single == first.single
            && r.sub_meta.len() == first.sub_meta.len()
            && r.buckets.len() == first.buckets.len()
            && r.buckets
                .iter()
                .zip(&first.buckets)
                .all(|(a, b)| a.0 == b.0)
    };
    if !rest.iter().all(same) {
        return None;
    }
    let mut merged = (*first).clone();
    for other in rest {
        for (into, from) in merged.buckets.iter_mut().zip(&other.buckets) {
            into.1 = into.1.saturating_add(from.1);
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
    // ALLOC: one box a `filters`, bounded by the request's.
    Some(Value::Filters(Box::new(merged)))
}
