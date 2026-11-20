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
            ..HistogramResult::default()
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

#[test]
fn a_histogram_reduces_each_keys_sub_aggregations_over_the_shards() {
    use oss_reader::aggs::bucket::sub_values::BucketSubs;
    use oss_reader::aggs::partial::Format;
    let with_subs = |buckets: &[(f64, u64, f64)]| {
        vec![(
            "h".to_string(),
            Value::Histogram(Box::new(HistogramResult {
                spec: spec(),
                buckets: buckets.iter().map(|&(k, c, _)| (k, c)).collect(),
                subs: BucketSubs {
                    meta: vec![("m".to_string(), Format::Raw)],
                    empty: vec![Value::Max(f64::NEG_INFINITY)],
                    lists: buckets
                        .iter()
                        .map(|&(_, _, m)| vec![Value::Max(m)])
                        .collect(),
                },
            })),
        )]
    };
    let out = reduce_shards(&[
        with_subs(&[(0.0, 1, 4.0), (10.0, 1, 9.0)]),
        with_subs(&[(10.0, 1, 12.0), (30.0, 1, 2.0)]),
    ])
    .expect("reduces");
    let [(_, Value::Histogram(h))] = out.as_slice() else {
        panic!("{out:?}");
    };
    // Key 20 is a gap the reduce fills with the empty value.
    assert_eq!(
        h.subs.lists,
        [
            vec![Value::Max(4.0)],
            vec![Value::Max(12.0)],
            vec![Value::Max(f64::NEG_INFINITY)],
            vec![Value::Max(2.0)]
        ]
    );
}
