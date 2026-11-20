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
