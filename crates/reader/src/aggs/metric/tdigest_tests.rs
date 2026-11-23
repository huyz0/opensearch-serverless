//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::*;

/// What OpenSearch 3.10.0's `TDigestState` answers, a line a case:
/// `kind n compression | quantiles | cdfs | reduced quantiles | reduced
/// cdfs | size reduced-size`, each double as its bits in hex. `kind` is
/// `lin` (`0..n`), `dup` (`i % 7`) or `rnd` (an LCG's values over 7); the
/// reduce adds the three digests of `i % 3` into an empty one.
const JAVA: &str = include_str!("tdigest_vectors.txt");

const PERCENTS: [f64; 10] = [0.0, 1.0, 5.0, 25.0, 33.3, 50.0, 75.0, 95.0, 99.0, 100.0];

fn values(kind: &str, n: usize) -> Vec<f64> {
    let mut s: i64 = 12345;
    (0..n)
        .map(|i| match kind {
            "lin" => i as f64,
            "dup" => (i % 7) as f64,
            _ => {
                s = s
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                ((s as u64 >> 11) % 100_000) as f64 / 7.0
            }
        })
        .collect()
}

fn built(values: impl Iterator<Item = f64>, compression: f64) -> Digest {
    let mut d = MergingDigest::new(compression);
    for v in values {
        d.add(v).expect("not NaN");
    }
    Digest::from_engine(d)
}

fn bits(d: f64) -> String {
    format!("{:x}", d.to_bits())
}

fn answers(d: &mut Digest, n: usize) -> (Vec<String>, Vec<String>) {
    d.settle();
    let q = PERCENTS
        .iter()
        .map(|p| bits(d.quantile(p / 100.0)))
        .collect();
    let n = n as f64;
    let xs = [
        -1.0,
        0.0,
        0.5,
        1.0,
        3.0,
        n / 2.0,
        n - 1.0,
        n,
        1e9,
        13.0 / 7.0,
    ];
    let c = xs.iter().map(|&x| bits(d.cdf(x))).collect();
    (q, c)
}

#[test]
fn quantiles_cdfs_and_reduces_match_opensearch() {
    let mut checked = 0;
    for line in JAVA.lines().filter(|l| !l.is_empty()) {
        let parts: Vec<&str> = line.split('|').map(str::trim).collect();
        let head: Vec<&str> = parts[0].split(' ').collect();
        let (kind, n, compression) = (
            head[0],
            head[1].parse::<usize>().expect("n"),
            head[2].parse::<f64>().expect("compression"),
        );
        let words = |i: usize| -> Vec<String> { parts[i].split(' ').map(str::to_owned).collect() };
        let v = values(kind, n);
        let mut one = built(v.iter().copied(), compression);
        let (q, c) = answers(&mut one, n);
        assert_eq!(q, words(1), "{kind} {n} {compression} quantiles");
        assert_eq!(c, words(2), "{kind} {n} {compression} cdfs");
        let mut reduced = Digest::new(compression);
        for part in 0..3 {
            let digest = built(
                v.iter()
                    .enumerate()
                    .filter(|(i, _)| i % 3 == part)
                    .map(|(_, x)| *x),
                compression,
            );
            reduced.add_digest(&digest);
        }
        let (q, c) = answers(&mut reduced, n);
        assert_eq!(q, words(3), "{kind} {n} {compression} reduced quantiles");
        assert_eq!(c, words(4), "{kind} {n} {compression} reduced cdfs");
        let sizes = words(5);
        assert_eq!(one.size().to_string(), sizes[0]);
        assert_eq!(reduced.size().to_string(), sizes[1]);
        checked += 1;
    }
    assert_eq!(checked, 48);
}

#[test]
fn an_empty_digest_answers_nan() {
    let mut d = Digest::new(100.0);
    d.settle();
    assert_eq!(d.size(), 0);
    assert!(d.quantile(0.5).is_nan());
    assert!(d.cdf(1.0).is_nan());
    let mut reduced = Digest::new(100.0);
    reduced.add_digest(&d);
    reduced.settle();
    assert!(reduced.quantile(0.5).is_nan());
}

#[test]
fn weighted_averages_stay_between_their_points() {
    assert_eq!(weighted_average(1.0, 1.0, 3.0, 1.0), 2.0);
    assert_eq!(weighted_average(3.0, 1.0, 1.0, 3.0), 1.5);
    assert!(weighted_average(1.0, 0.0, 1.0, 0.0).is_nan());
}

#[test]
fn a_merge_keeps_the_centroid_arrays_and_grows_only_short_ones() {
    let v = values("rnd", 3000);
    let mut d = built(v.iter().copied(), 100.0);
    let size = d.mean.len();
    d.settle();
    assert_eq!((d.mean.len(), d.weight.len()), (size, size));
    // Arrays shorter than the merge needs grow one cell at a time, to the
    // same answers. Fewer values than the buffer holds, so the engine has
    // not merged yet.
    let few = &v[..500];
    let mut engine = MergingDigest::new(100.0);
    for x in few {
        engine.add(*x).expect("not NaN");
    }
    assert_eq!(engine.last_used_cell, 0);
    engine.mean.truncate(1);
    engine.weight.truncate(1);
    let mut short = Digest::from_engine(engine);
    let mut full = built(few.iter().copied(), 100.0);
    short.settle();
    full.settle();
    assert_eq!(short.quantile(0.5).to_bits(), full.quantile(0.5).to_bits());
    assert_eq!(short.cdf(5000.0).to_bits(), full.cdf(5000.0).to_bits());
    assert_eq!(short.last_used_cell, full.last_used_cell);
    assert!(full.last_used_cell > 1);
    assert_eq!(short.mean.len(), short.last_used_cell);
}

/// A settled digest of centroids set by hand, as `merge` leaves them.
fn settled(means: &[f64], weights: &[f64], min: f64, max: f64) -> Digest {
    let mut d = Digest::new(100.0);
    d.mean = means.to_vec();
    d.weight = weights.to_vec();
    d.last_used_cell = means.len();
    d.total_weight = weights.iter().sum();
    d.min = min;
    d.max = max;
    d
}

/// `quantile` of five centroids whose two outer ones weigh more than one,
/// at every half value of its index in `0..=16` (`q * total`, exact as the
/// total is 16). The answers are `MergingDigest.quantile`'s formulas worked
/// out by an independent script: the interpolation toward the smallest
/// value, the one between centroids, the one toward the largest, and the
/// edges between them.
#[test]
fn quantile_interpolates_at_the_ends_and_between_centroids() {
    let d = settled(
        &[10.0, 20.0, 30.0, 40.0, 50.0],
        &[4.0, 3.0, 2.0, 3.0, 4.0],
        2.0,
        58.0,
    );
    let expected: [(f64, f64); 33] = [
        (0.0, 2.0),
        (0.5, 2.0),
        (1.0, 2.0),
        (1.5, 6.0),
        (2.0, 10.0),
        (2.5, 11.428571428571429),
        (3.0, 12.857142857142858),
        (3.5, 14.285714285714286),
        (4.0, 15.714285714285714),
        (4.5, 17.142857142857142),
        (5.0, 18.571428571428573),
        (5.5, 20.0),
        (6.0, 22.0),
        (6.5, 24.0),
        (7.0, 26.0),
        (7.5, 28.0),
        (8.0, 30.0),
        (8.5, 32.0),
        (9.0, 34.0),
        (9.5, 36.0),
        (10.0, 38.0),
        (10.5, 40.0),
        (11.0, 41.42857142857143),
        (11.5, 42.857142857142854),
        (12.0, 44.285714285714285),
        (12.5, 45.714285714285715),
        (13.0, 47.142857142857146),
        (13.5, 48.57142857142857),
        (14.0, 50.0),
        (14.5, 54.0),
        (15.0, 58.0),
        (15.5, 58.0),
        (16.0, 58.0),
    ];
    for (index, answer) in expected {
        assert_eq!(d.quantile(index / 16.0), answer, "index {index}");
    }
}

#[test]
fn size_counts_the_merged_and_the_buffered_values() {
    for n in [3usize, 40, 1000, 3000] {
        let d = built((0..n).map(|i| i as f64), 100.0);
        assert_eq!(d.size(), n as u64, "{n}");
    }
}

#[test]
fn adding_a_digest_keeps_the_smallest_and_the_largest_value_like_java() {
    let mut other = Digest::new(100.0);
    other.mean = vec![-0.0, 7.0];
    other.weight = vec![1.0, 1.0];
    other.last_used_cell = 2;
    other.total_weight = 2.0;
    other.min = -0.0;
    other.max = 7.0;
    // A tie keeps the value there: `0.0` stays, `-0.0` does not replace it.
    let mut d = built([0.0, 7.0].into_iter(), 100.0);
    d.add_digest(&other);
    assert_eq!(d.min.to_bits(), 0.0f64.to_bits());
    assert_eq!(d.max, 7.0);
    let mut d = Digest::new(100.0);
    d.add_digest(&other);
    assert_eq!((d.min, d.max), (0.0, 7.0));
    let mut wide = built([-3.0, 11.0].into_iter(), 100.0);
    wide.add_digest(&other);
    assert_eq!((wide.min, wide.max), (-3.0, 11.0));
}

#[test]
fn a_merge_runs_only_for_buffered_values_or_when_forced() {
    let mut empty = Digest::new(100.0);
    empty.compress();
    assert_eq!(empty.merge_count, 0);
    let mut d = built((0..50).map(f64::from), 100.0);
    d.settle();
    let after_first = d.merge_count;
    d.settle();
    d.settle();
    assert_eq!(d.merge_count, after_first);
    d.compress();
    assert_eq!(d.merge_count, after_first + 1);
}

#[test]
fn a_merge_takes_the_smallest_and_largest_from_its_centroids() {
    let mut d = Digest::new(100.0);
    for (i, x) in [3.0, 1.0, 2.0].into_iter().enumerate() {
        d.temp_mean[i] = x;
        d.temp_weight[i] = 1.0;
    }
    d.temp_used = 3;
    d.unmerged_weight = 3.0;
    d.settle();
    assert_eq!((d.min, d.max), (1.0, 3.0));
}

/// Outer centroids of weight 6, whose half less one is 2, so a divide that multiplies, or takes a remainder, answers otherwise.
#[test]
fn quantile_divides_the_ends_by_half_their_weight_less_one() {
    let d = settled(&[10.0, 20.0, 30.0], &[6.0, 4.0, 6.0], 2.0, 58.0);
    let expected: [(f64, f64); 33] = [
        (0.0, 2.0),
        (0.5, 2.0),
        (1.0, 2.0),
        (1.5, 4.0),
        (2.0, 6.0),
        (2.5, 8.0),
        (3.0, 10.0),
        (3.5, 11.0),
        (4.0, 12.0),
        (4.5, 13.0),
        (5.0, 14.0),
        (5.5, 15.0),
        (6.0, 16.0),
        (6.5, 17.0),
        (7.0, 18.0),
        (7.5, 19.0),
        (8.0, 20.0),
        (8.5, 21.0),
        (9.0, 22.0),
        (9.5, 23.0),
        (10.0, 24.0),
        (10.5, 25.0),
        (11.0, 26.0),
        (11.5, 27.0),
        (12.0, 28.0),
        (12.5, 29.0),
        (13.0, 30.0),
        (13.5, 37.0),
        (14.0, 44.0),
        (14.5, 51.0),
        (15.0, 58.0),
        (15.5, 58.0),
        (16.0, 58.0),
    ];
    for (index, answer) in expected {
        assert_eq!(d.quantile(index / 16.0), answer, "index {index}");
    }
}

/// A last centroid of weight 1: the index `total - 1` is interpolated, not the largest value.
#[test]
fn quantile_at_the_last_but_one_value_of_a_singleton_end_is_not_the_largest() {
    let d = settled(
        &[10.0, 20.0, 30.0, 40.0, 50.0],
        &[4.0, 4.0, 3.0, 4.0, 1.0],
        2.0,
        58.0,
    );
    let expected: [(f64, f64); 33] = [
        (0.0, 2.0),
        (0.5, 2.0),
        (1.0, 2.0),
        (1.5, 6.0),
        (2.0, 10.0),
        (2.5, 11.25),
        (3.0, 12.5),
        (3.5, 13.75),
        (4.0, 15.0),
        (4.5, 16.25),
        (5.0, 17.5),
        (5.5, 18.75),
        (6.0, 20.0),
        (6.5, 21.428571428571427),
        (7.0, 22.857142857142858),
        (7.5, 24.285714285714285),
        (8.0, 25.714285714285715),
        (8.5, 27.142857142857142),
        (9.0, 28.571428571428573),
        (9.5, 30.0),
        (10.0, 31.428571428571427),
        (10.5, 32.857142857142854),
        (11.0, 34.285714285714285),
        (11.5, 35.714285714285715),
        (12.0, 37.142857142857146),
        (12.5, 38.57142857142857),
        (13.0, 40.0),
        (13.5, 42.5),
        (14.0, 45.0),
        (14.5, 47.5),
        (15.0, 50.0),
        (15.5, 58.0),
        (16.0, 58.0),
    ];
    for (index, answer) in expected {
        assert_eq!(d.quantile(index / 16.0), answer, "index {index}");
    }
}

#[test]
fn a_merge_grows_buffers_shorter_than_it_needs() {
    let v = values("rnd", 400);
    let mut whole = built(v.iter().copied(), 100.0);
    whole.settle();
    let mut short = whole.clone();
    short.temp_mean.clear();
    short.temp_weight.clear();
    whole.compress();
    short.compress();
    assert_eq!(short.mean, whole.mean);
    assert_eq!(short.weight, whole.weight);
    assert_eq!(short.quantile(0.9).to_bits(), whole.quantile(0.9).to_bits());
}
