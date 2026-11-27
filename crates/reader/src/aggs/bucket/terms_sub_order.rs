//! A numeric `terms` ordered by a metric sub-aggregation
//! (`InternalOrder.Aggregation`): the shard keeps every bucket until it has
//! counted them all, then orders and cuts them as `BucketSelectionStrategy`
//! does; the reduce orders the merged buckets by the reduced value.

use crate::aggs::bucket::sub_order::{self, SubOrder};
use crate::aggs::bucket::sub_values::BucketSubs;
use crate::aggs::bucket::terms::TermsResult;

/// The indexes of `keys` in the order of `order`, by the value each bucket's
/// list in `subs` holds of the targeted sub-aggregation, ties by key
/// ascending. A bucket with no list sorts as NaN, last.
pub fn rank(order: &SubOrder, subs: &BucketSubs, keys: &[i64]) -> Vec<usize> {
    let target = sub_order::target(order, subs.meta.iter().map(|m| m.0.as_str()));
    // ALLOC: one value a bucket, bounded by the buckets.
    let values: Vec<f64> = (0..keys.len())
        .map(|at| {
            target
                .as_ref()
                .zip(subs.lists.get(at))
                .map_or(f64::NAN, |(t, list)| t.sort(list))
        })
        // ALLOC: as above.
        .collect();
    sub_order::ranked(order, &values, keys)
}

/// `items` taken in the order `at` names, each at most once.
pub fn take_in_order<T>(items: Vec<T>, at: &[usize]) -> Vec<T> {
    // ALLOC: one slot a bucket, bounded by the buckets.
    let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
    at.iter()
        .filter_map(|&i| slots.get_mut(i).and_then(Option::take))
        // ALLOC: one entry a bucket, as above.
        .collect()
}

impl TermsResult {
    /// Orders the buckets by the spec's sub-aggregation, keeps the best
    /// `shard_size` and counts the others' documents as other
    /// (`BucketSelectionStrategy`), once the sub-aggregation lists are in
    /// place and the shard minimum has applied.
    pub fn order_by_sub(&mut self) {
        let Some(order) = self.spec.sub_order.clone() else {
            return;
        };
        // ALLOC: one key a bucket, bounded by the buckets.
        let keys: Vec<i64> = self.buckets.iter().map(|b| b.0).collect();
        let at = rank(&order, &self.subs, &keys);
        let keep = self.spec.shard_size.min(at.len());
        let cut: u64 = at
            .iter()
            .skip(keep)
            .filter_map(|&i| self.buckets.get(i))
            .map(|b| b.1)
            .sum();
        self.other_doc_count = self.other_doc_count.saturating_add(cut);
        let kept = at.get(..keep).unwrap_or(&at);
        self.buckets = take_in_order(std::mem::take(&mut self.buckets), kept);
        if self.subs.lists.len() == keys.len() {
            self.subs.lists = take_in_order(std::mem::take(&mut self.subs.lists), kept);
        }
    }
}
