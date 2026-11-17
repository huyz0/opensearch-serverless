//! A `percentiles` or `percentile_ranks` aggregation's shard-level state:
//! OpenSearch's `TDigestState`, a t-digest 3.3 `MergingDigest`, taken over
//! from the one lucene-aggs builds, with what the response and the reduce
//! ask of it: `quantile`, `cdf` and `add(TDigest)` (`AbstractTDigest.add`),
//! ported from t-digest 3.3's `MergingDigest` and `AbstractTDigest`.
//!
//! lucene-aggs builds the digest value by value as Java does and exposes
//! its arrays; what it keeps private is the merge, which a quantile and a
//! reduce run again, so it is ported here too: `mergeNewValues` and `merge`
//! under the `K_2` scale function with the weight limit, every other merge
//! run backwards.

use lucene_aggs::tdigest::MergingDigest;
use lucene_search::java_math::{java_max, java_min};

/// One bucket's digest.
#[derive(Clone, Debug, PartialEq)]
pub struct Digest {
    /// `compression` as asked: what a reduce makes its digest with.
    pub public_compression: f64,
    compression: f64,
    merge_count: i32,
    last_used_cell: usize,
    total_weight: f64,
    weight: Vec<f64>,
    mean: Vec<f64>,
    unmerged_weight: f64,
    temp_used: usize,
    temp_weight: Vec<f64>,
    temp_mean: Vec<f64>,
    min: f64,
    max: f64,
}

impl Digest {
    /// The digest lucene-aggs built, taken over whole.
    pub fn from_engine(d: MergingDigest) -> Digest {
        Digest {
            public_compression: d.public_compression,
            compression: d.compression,
            merge_count: d.merge_count,
            last_used_cell: d.last_used_cell,
            total_weight: d.total_weight,
            weight: d.weight,
            mean: d.mean,
            unmerged_weight: d.unmerged_weight,
            temp_used: d.temp_used,
            temp_weight: d.temp_weight,
            temp_mean: d.temp_mean,
            min: d.min,
            max: d.max,
        }
    }

    /// `new TDigestState(compression)`.
    pub fn new(compression: f64) -> Digest {
        Digest::from_engine(MergingDigest::new(compression))
    }

    /// `size()`: how many values went in.
    pub fn size(&self) -> u64 {
        // ARITH: none; `(long)` of a non-negative whole weight, saturating.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let size = (self.total_weight + self.unmerged_weight) as u64;
        size
    }

    /// `add(x, w)`: a centroid into the buffer, merged first when it is full.
    fn add_weighted(&mut self, x: f64, w: f64) {
        let room = self
            .temp_weight
            .len()
            .saturating_sub(self.last_used_cell)
            .saturating_sub(1);
        if self.temp_used >= room {
            self.merge_new_values(false, self.compression);
        }
        let at = self.temp_used;
        if let (Some(tw), Some(tm)) = (self.temp_weight.get_mut(at), self.temp_mean.get_mut(at)) {
            *tw = w;
            *tm = x;
        }
        self.temp_used = at.saturating_add(1);
        self.unmerged_weight += w;
        if x < self.min {
            self.min = x;
        }
        if x > self.max {
            self.max = x;
        }
    }

    /// `AbstractTDigest.add(TDigest other)`: every centroid of `other`,
    /// compressed first (`centroids()`), added with its count.
    pub fn add_digest(&mut self, other: &Digest) {
        let mut other = other.clone();
        other.compress();
        for i in 0..other.last_used_cell {
            let (Some(&m), Some(&w)) = (other.mean.get(i), other.weight.get(i)) else {
                break;
            };
            // `centroid.count()`: an int of the weight.
            // ARITH: none; a centroid's weight is a whole number of values.
            #[allow(clippy::cast_possible_truncation)]
            let count = f64::from(w as i32);
            // `new Centroid(mean, count)` keeps `0 + count * (mean - 0) /
            // count`, which is not always `mean` to the last bit.
            let centroid = 0.0 + count * (m - 0.0) / count;
            self.add_weighted(centroid, count);
        }
    }

    /// `compress()`: `mergeNewValues(true, publicCompression)`.
    fn compress(&mut self) {
        self.merge_new_values(true, self.public_compression);
    }

    /// `mergeNewValues(force, compression)`.
    fn merge_new_values(&mut self, force: bool, compression: f64) {
        if self.total_weight == 0.0 && self.unmerged_weight == 0.0 {
            return;
        }
        if force || self.unmerged_weight > 0.0 {
            let backwards = self.merge_count % 2 == 1;
            self.merge(self.unmerged_weight, backwards, compression);
            self.merge_count = self.merge_count.wrapping_add(1);
            self.temp_used = 0;
            self.unmerged_weight = 0.0;
        }
    }

    /// `merge(tempMean, tempWeight, tempUsed, null, order, unmergedWeight,
    /// runBackwards, compression)`.
    fn merge(&mut self, unmerged_weight: f64, backwards: bool, compression: f64) {
        let last = self.last_used_cell;
        let fresh = self.temp_used;
        let count = fresh.saturating_add(last);
        // The buffer always has room for the merged centroids (`add`'s
        // check); grow it if a reduce's digest somehow lacks it.
        if self.temp_mean.len() < count {
            self.temp_mean.resize(count, 0.0);
            self.temp_weight.resize(count, 0.0);
        }
        for i in 0..last {
            let at = fresh.saturating_add(i);
            if let (Some(t), Some(&m)) = (self.temp_mean.get_mut(at), self.mean.get(i)) {
                *t = m;
            }
            if let (Some(t), Some(&w)) = (self.temp_weight.get_mut(at), self.weight.get(i)) {
                *t = w;
            }
        }
        // `Sort.stableSort`: by value then index, -0.0 tied with 0.0.
        // ALLOC: one index a centroid and buffered value, bounded by the buffer.
        let mut order: Vec<usize> = (0..count).collect();
        let values = &self.temp_mean;
        // Unstable, but the index breaks every tie, so the order is the
        // stable one, and no scratch buffer is allocated.
        order.sort_unstable_by(|&a, &b| {
            let (x, y) = (
                values.get(a).copied().unwrap_or(0.0),
                values.get(b).copied().unwrap_or(0.0),
            );
            if x < y {
                std::cmp::Ordering::Less
            } else if x > y {
                std::cmp::Ordering::Greater
            } else {
                a.cmp(&b)
            }
        });
        self.total_weight += unmerged_weight;
        if backwards {
            order.reverse();
        }
        let total = self.total_weight;
        let tm = |i: usize| self.temp_mean.get(i).copied().unwrap_or(0.0);
        let Some(&first) = order.first() else { return };
        let mut mean = std::mem::take(&mut self.mean);
        let mut weight = std::mem::take(&mut self.weight);
        let mut cell = 0usize;
        grow_to(&mut mean, &mut weight, cell);
        set(&mut mean, 0, tm(first));
        set(
            &mut weight,
            0,
            self.temp_weight.get(first).copied().unwrap_or(0.0),
        );
        let mut w_so_far = 0.0;
        // `ScaleFunction.K_2.normalizer`: compression / Z(compression, n).
        let normalizer = compression / (4.0 * (total / compression).ln() + 24.0);
        let max_q = |q: f64| q * (1.0 - q) / normalizer;
        for (i, &ix) in order.iter().enumerate().skip(1) {
            let incoming = self.temp_weight.get(ix).copied().unwrap_or(0.0);
            let here = weight.get(cell).copied().unwrap_or(0.0);
            let proposed = here + incoming;
            let q0 = w_so_far / total;
            let q2 = (w_so_far + proposed) / total;
            let mut add_this = proposed <= total * java_min(max_q(q0), max_q(q2));
            if i == 1 || i == count.saturating_sub(1) {
                add_this = false;
            }
            if add_this {
                set(&mut weight, cell, here + incoming);
                let m = mean.get(cell).copied().unwrap_or(0.0);
                set(
                    &mut mean,
                    cell,
                    m + (tm(ix) - m) * incoming / (here + incoming),
                );
            } else {
                w_so_far += here;
                cell = cell.saturating_add(1);
                grow_to(&mut mean, &mut weight, cell);
                set(&mut mean, cell, tm(ix));
                set(&mut weight, cell, incoming);
            }
            set(&mut self.temp_weight, ix, 0.0);
        }
        cell = cell.saturating_add(1);
        if backwards {
            if let Some(m) = mean.get_mut(..cell) {
                m.reverse();
            }
            if let Some(w) = weight.get_mut(..cell) {
                w.reverse();
            }
        }
        if total > 0.0 {
            self.min = java_min(self.min, mean.first().copied().unwrap_or(f64::INFINITY));
            self.max = java_max(
                self.max,
                mean.get(cell.saturating_sub(1))
                    .copied()
                    .unwrap_or(f64::NEG_INFINITY),
            );
        }
        self.mean = mean;
        self.weight = weight;
        self.last_used_cell = cell;
    }

    fn m(&self, i: usize) -> f64 {
        self.mean.get(i).copied().unwrap_or(f64::NAN)
    }

    fn w(&self, i: usize) -> f64 {
        self.weight.get(i).copied().unwrap_or(0.0)
    }

    /// `mergeNewValues()`, what `quantile` and `cdf` run first: every
    /// buffered value merged into the centroids. Run once before asking
    /// [`quantile`](Self::quantile) or [`cdf`](Self::cdf), which do not
    /// see values still buffered; running it again changes nothing.
    pub fn settle(&mut self) {
        self.merge_new_values(false, self.compression);
    }

    /// `quantile(q)` of a settled digest, `q` in `[0, 1]`; NaN for an
    /// empty digest.
    pub fn quantile(&self, q: f64) -> f64 {
        let n = self.last_used_cell;
        if n == 0 {
            return f64::NAN;
        }
        if n == 1 {
            return self.m(0);
        }
        let total = self.total_weight;
        let index = q * total;
        if index < 1.0 {
            return self.min;
        }
        let (w0, m0) = (self.w(0), self.m(0));
        if w0 > 1.0 && index < w0 / 2.0 {
            return self.min + (index - 1.0) / (w0 / 2.0 - 1.0) * (m0 - self.min);
        }
        if index > total - 1.0 {
            return self.max;
        }
        let last = n.saturating_sub(1);
        let (wn, mn) = (self.w(last), self.m(last));
        if wn > 1.0 && total - index <= wn / 2.0 {
            return self.max - (total - index - 1.0) / (wn / 2.0 - 1.0) * (self.max - mn);
        }
        let mut weight_so_far = w0 / 2.0;
        for i in 0..last {
            let next = i.saturating_add(1);
            let dw = (self.w(i) + self.w(next)) / 2.0;
            if weight_so_far + dw > index {
                let mut left_unit = 0.0;
                if self.w(i) == 1.0 {
                    if index - weight_so_far < 0.5 {
                        return self.m(i);
                    }
                    left_unit = 0.5;
                }
                let mut right_unit = 0.0;
                if self.w(next) == 1.0 {
                    if weight_so_far + dw - index <= 0.5 {
                        return self.m(next);
                    }
                    right_unit = 0.5;
                }
                let z1 = index - weight_so_far - left_unit;
                let z2 = weight_so_far + dw - index - right_unit;
                return weighted_average(self.m(i), z2, self.m(next), z1);
            }
            weight_so_far += dw;
        }
        let z1 = index - total - wn / 2.0;
        let z2 = wn / 2.0 - z1;
        weighted_average(mn, z1, self.max, z2)
    }

    /// `cdf(x)` of a settled digest and a finite `x`: the fraction of
    /// values at or below it,
    /// NaN for an empty digest.
    pub fn cdf(&self, x: f64) -> f64 {
        let n = self.last_used_cell;
        let (min, max, total) = (self.min, self.max, self.total_weight);
        if n == 0 {
            return f64::NAN;
        }
        if n == 1 {
            let width = max - min;
            return if x < min {
                0.0
            } else if x > max {
                1.0
            } else if x - min <= width {
                0.5
            } else {
                (x - min) / (max - min)
            };
        }
        if x < min {
            return 0.0;
        }
        if x > max {
            return 1.0;
        }
        let (m0, w0) = (self.m(0), self.w(0));
        if x < m0 {
            if m0 - min > 0.0 {
                if x == min {
                    return 0.5 / total;
                }
                return (1.0 + (x - min) / (m0 - min) * (w0 / 2.0 - 1.0)) / total;
            }
            return 0.0;
        }
        let last = n.saturating_sub(1);
        let (mn, wn) = (self.m(last), self.w(last));
        if x > mn {
            if max - mn > 0.0 {
                if x == max {
                    return 1.0 - 0.5 / total;
                }
                let dq = (1.0 + (max - x) / (max - mn) * (wn / 2.0 - 1.0)) / total;
                return 1.0 - dq;
            }
            return 1.0;
        }
        let mut weight_so_far = 0.0;
        let mut it = 0usize;
        while it < last {
            let next = it.saturating_add(1);
            if self.m(it) == x {
                let mut dw = 0.0;
                while it < n && self.m(it) == x {
                    dw += self.w(it);
                    it = it.saturating_add(1);
                }
                return (weight_so_far + dw / 2.0) / total;
            } else if self.m(it) <= x && x < self.m(next) {
                if self.m(next) - self.m(it) > 0.0 {
                    let mut left_excluded = 0.0;
                    let mut right_excluded = 0.0;
                    if self.w(it) == 1.0 {
                        if self.w(next) == 1.0 {
                            return (weight_so_far + 1.0) / total;
                        }
                        left_excluded = 0.5;
                    } else if self.w(next) == 1.0 {
                        right_excluded = 0.5;
                    }
                    let dw = (self.w(it) + self.w(next)) / 2.0;
                    let (left, right) = (self.m(it), self.m(next));
                    let dw_no_singleton = dw - left_excluded - right_excluded;
                    let base = weight_so_far + self.w(it) / 2.0 + left_excluded;
                    return (base + dw_no_singleton * (x - left) / (right - left)) / total;
                }
                let dw = (self.w(it) + self.w(next)) / 2.0;
                return (weight_so_far + dw) / total;
            }
            weight_so_far += self.w(it);
            it = next;
        }
        // `x == mean[n - 1]`; Java throws "Can't happen" otherwise.
        1.0 - 0.5 / total
    }
}

/// Room for centroid `cell`. The weight limit keeps the cells within the
/// arrays `MergingDigest` sizes (Java indexes them unchecked), so this
/// never grows them in practice.
fn grow_to(mean: &mut Vec<f64>, weight: &mut Vec<f64>, cell: usize) {
    if cell >= mean.len() {
        // ALLOC: one cell past the constructor's size, which the weight limit never reaches.
        mean.push(0.0);
        weight.push(0.0);
    }
}

fn set(values: &mut [f64], at: usize, value: f64) {
    if let Some(v) = values.get_mut(at) {
        *v = value;
    }
}

/// The most bytes building, settling and reading one digest of
/// `compression` holds, sized as `MergingDigest`'s constructor sizes its
/// arrays: its centroids' means and weights, its buffer's, the engine's
/// sort order, the engine's per-thread sort keys (two radix buffers and a
/// stable-sort buffer, which it keeps for the thread's next digest), and
/// the order a merge here sorts.
pub fn digest_bytes(compression: f64) -> usize {
    let compression = if compression < 10.0 {
        10.0
    } else {
        compression
    };
    let fudge = if compression < 30.0 { 30.0 } else { 10.0 };
    // ARITH: none; f64 throughout, and `as` saturates.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bytes = {
        let mut size = (2.0 * compression + fudge).floor();
        let mut buffer = 5.0 * size;
        // `scale`: `max(1, bufferSize / size - 1)` with an int division, 4.
        let working = 2.0 * compression;
        if size < working + fudge {
            size = (working + fudge).ceil();
        }
        if buffer <= 2.0 * size {
            buffer = 2.0 * size;
        }
        let centroids = size * 16.0;
        // A buffered slot: its mean and weight (8 bytes each), the engine's
        // u32 order, its two `(u64, u32)` radix keys and `u128` sort key
        // (16 bytes each), and the merge's usize order here.
        let buffered = buffer * (8.0 + 8.0 + 4.0 + 16.0 * 3.0 + 8.0);
        (centroids + buffered) as usize
    };
    bytes.saturating_add(std::mem::size_of::<Digest>())
}

/// `AbstractTDigest.weightedAverage`: between the two, whichever is lower
/// first, clamped to them.
fn weighted_average(x1: f64, w1: f64, x2: f64, w2: f64) -> f64 {
    let (x1, w1, x2, w2) = if x1 <= x2 {
        (x1, w1, x2, w2)
    } else {
        (x2, w2, x1, w1)
    };
    let x = (x1 * w1 + x2 * w2) / (w1 + w2);
    java_max(x1, java_min(x, x2))
}

#[cfg(test)]
#[path = "tdigest_tests.rs"]
mod tests;
