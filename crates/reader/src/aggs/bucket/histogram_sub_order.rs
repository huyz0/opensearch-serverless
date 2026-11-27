//! A `histogram` or `date_histogram` ordered by a metric sub-aggregation
//! (`InternalOrder.Aggregation`): the reduce has the merged buckets by key
//! ascending with their reduced sub-aggregations, and lists them by the
//! target's value, ties by key ascending (`BucketOrder.compound`'s
//! tie-breaker); a bucket with no value sorts as NaN, last.

use crate::aggs::bucket::sub_order::SubOrder;
use crate::aggs::bucket::sub_values::BucketSubs;
use crate::aggs::bucket::terms_sub_order::{rank, take_in_order};

/// `buckets` and their `subs` (one list a bucket, by key ascending) in the
/// order of `order`.
pub fn by_sub<K>(
    order: &SubOrder,
    buckets: Vec<(K, u64)>,
    mut subs: BucketSubs,
) -> (Vec<(K, u64)>, BucketSubs) {
    // ALLOC: one position a bucket, bounded by the bucket limit.
    let positions: Vec<i64> = (0..buckets.len())
        .map(|at| i64::try_from(at).unwrap_or(i64::MAX))
        // ALLOC: as above.
        .collect();
    let at = rank(order, &subs, &positions);
    if subs.lists.len() == buckets.len() {
        subs.lists = take_in_order(std::mem::take(&mut subs.lists), &at);
    }
    (take_in_order(buckets, &at), subs)
}
