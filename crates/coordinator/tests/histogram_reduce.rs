//! docs/specs/histogram-agg AC-5: a `histogram` reduces across shards
//! through the coordinator's `reduce_shards`.

use oss_coordinator::reduce::aggs::{AggsError, reduce_shards};
use oss_reader::aggs::bucket::histogram::{HistogramResult, HistogramSpec};
use oss_reader::aggs::partial::Value;

fn spec() -> HistogramSpec {
    HistogramSpec {
        interval: 10.0,
        ..HistogramSpec::default()
    }
}

fn shard(buckets: &[(f64, u64)]) -> Vec<(String, Value)> {
    vec![(
        "h".to_string(),
        Value::Histogram(Box::new(HistogramResult {
            spec: spec(),
            buckets: buckets.to_vec(),
        })),
    )]
}

#[test]
fn a_histogram_reduces_over_the_shards_that_answered() {
    let out = reduce_shards(&[
        shard(&[(0.0, 5), (10.0, 3)]),
        shard(&[(10.0, 4), (40.0, 2)]),
    ])
    .expect("reduces");
    let [(name, Value::Histogram(h))] = out.as_slice() else {
        panic!("{out:?}");
    };
    assert_eq!(name, "h");
    assert_eq!(
        h.buckets,
        [(0.0, 5), (10.0, 7), (20.0, 0), (30.0, 0), (40.0, 2)]
    );
}

#[test]
fn a_histogram_and_another_kind_do_not_reduce() {
    let mut other = shard(&[]);
    other[0].1 = Value::Min(1.0);
    let err = reduce_shards(&[shard(&[(0.0, 1)]), other]).expect_err("mismatched");
    assert_eq!(err, AggsError::Mismatched { name: "h".into() });
}
