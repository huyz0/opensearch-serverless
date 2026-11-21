//! The sort OpenSearch runs over a histogram's buckets ordered by count.

use std::cmp::Ordering;

use lucene_util::sorter::{IntroTarget, intro_sort};

/// `CollectionUtil.introSort` over `(key, doc count)` buckets by count: the
/// same unstable sort, so equal counts land where OpenSearch puts them.
struct ByCount<'a, K> {
    buckets: &'a mut [(K, u64)],
    ascending: bool,
    pivot: u64,
}

impl<K> IntroTarget for ByCount<'_, K> {
    fn swap(&mut self, i: usize, j: usize) {
        self.buckets.swap(i, j);
    }
    fn set_pivot(&mut self, i: usize) {
        self.pivot = self.buckets.get(i).map_or(0, |b| b.1);
    }
    fn compare_pivot(&mut self, j: usize) -> i32 {
        let other = self.buckets.get(j).map_or(0, |b| b.1);
        let ordering = if self.ascending {
            self.pivot.cmp(&other)
        } else {
            other.cmp(&self.pivot)
        };
        match ordering {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }
}

/// Orders `buckets` by doc count, ascending or descending.
pub fn sort_by_count<K>(buckets: &mut [(K, u64)], ascending: bool) {
    let len = buckets.len();
    intro_sort(
        &mut ByCount {
            buckets,
            ascending,
            pivot: 0,
        },
        0,
        len,
    );
}
