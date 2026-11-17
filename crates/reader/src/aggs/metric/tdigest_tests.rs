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
