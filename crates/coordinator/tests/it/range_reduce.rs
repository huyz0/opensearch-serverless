//! docs/specs/range-agg AC-5: a `range` reduces across shards through the
//! coordinator's `reduce_shards`.

use oss_coordinator::reduce::aggs::reduce_shards;
use oss_reader::aggs::partial::Value;

#[test]
fn a_range_adds_its_counts_over_the_shards_that_answered() {
    use oss_reader::aggs::bucket::range::{RangeBucket, RangeResult};
    let range = |counts: [u64; 2]| {
        vec![(
            "r".to_string(),
            Value::Range(Box::new(RangeResult {
                keyed: true,
                buckets: vec![
                    RangeBucket {
                        key: None,
                        from: f64::NEG_INFINITY,
                        to: 5.0,
                        doc_count: counts[0],
                    },
                    RangeBucket {
                        key: Some("hi".to_string()),
                        from: 5.0,
                        to: f64::INFINITY,
                        doc_count: counts[1],
                    },
                ],
                ..RangeResult::default()
            })),
        )]
    };
    let out = reduce_shards(&[range([1, 2]), range([30, 40])]).expect("reduces");
    let [(name, Value::Range(r))] = out.as_slice() else {
        panic!("{out:?}");
    };
    assert_eq!(name, "r");
    assert!(r.keyed);
    let counts: Vec<u64> = r.buckets.iter().map(|b| b.doc_count).collect();
    assert_eq!(counts, [31, 42]);
    assert_eq!(r.buckets[1].key.as_deref(), Some("hi"));
}

#[test]
fn a_range_reduces_each_buckets_sub_aggregations_over_the_shards() {
    use oss_reader::aggs::bucket::range::{RangeBucket, RangeResult};
    use oss_reader::aggs::partial::Format;
    let range = |sums: [f64; 2]| {
        vec![(
            "r".to_string(),
            Value::Range(Box::new(RangeResult {
                buckets: [(f64::NEG_INFINITY, 5.0), (5.0, f64::INFINITY)]
                    .map(|(from, to)| RangeBucket {
                        key: None,
                        from,
                        to,
                        doc_count: 1,
                    })
                    .to_vec(),
                sub_meta: vec![("s".to_string(), Format::Raw)],
                subs: sums.iter().map(|s| vec![Value::Sum(*s)]).collect(),
                ..RangeResult::default()
            })),
        )]
    };
    let out = reduce_shards(&[range([1.0, 2.0]), range([10.0, 20.0])]).expect("reduces");
    let [(_, Value::Range(r))] = out.as_slice() else {
        panic!("{out:?}");
    };
    assert_eq!(r.subs, [vec![Value::Sum(11.0)], vec![Value::Sum(22.0)]]);
}
