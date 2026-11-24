//! docs/specs/date-histogram-agg AC-5: a `date_histogram` reduces across
//! shards through the coordinator's `reduce_shards`.

use oss_coordinator::reduce::aggs::{AggsError, reduce_shards};
use oss_reader::aggs::bucket::date_histogram::{
    DateHistogramResult, DateHistogramSpec, DateRounding, DateUnit, RoundingKind,
};
use oss_reader::aggs::partial::Value;

const DAY: i64 = 86_400_000;

fn spec() -> DateHistogramSpec {
    DateHistogramSpec {
        rounding: DateRounding {
            kind: RoundingKind::Unit(DateUnit::Day),
            zone_ms: 0,
            region: None,
            offset: 0,
        },
        ..DateHistogramSpec::default()
    }
}

fn shard(buckets: &[(i64, u64)]) -> Vec<(String, Value)> {
    vec![(
        "h".to_string(),
        Value::DateHistogram(Box::new(DateHistogramResult {
            spec: spec(),
            buckets: buckets.to_vec(),
            ..DateHistogramResult::default()
        })),
    )]
}

#[test]
fn a_date_histogram_reduces_over_the_shards_that_answered() {
    let out = reduce_shards(&[shard(&[(0, 5), (DAY, 3)]), shard(&[(DAY, 4), (4 * DAY, 2)])])
        .expect("reduces");
    let [(name, Value::DateHistogram(h))] = out.as_slice() else {
        panic!("{out:?}");
    };
    assert_eq!(name, "h");
    assert_eq!(
        h.buckets,
        [(0, 5), (DAY, 7), (2 * DAY, 0), (3 * DAY, 0), (4 * DAY, 2)]
    );
}

#[test]
fn a_date_histogram_and_another_kind_do_not_reduce() {
    let mut other = shard(&[]);
    other[0].1 = Value::Min(1.0);
    let err = reduce_shards(&[shard(&[(0, 1)]), other]).expect_err("mismatched");
    assert_eq!(err, AggsError::Mismatched { name: "h".into() });
}
