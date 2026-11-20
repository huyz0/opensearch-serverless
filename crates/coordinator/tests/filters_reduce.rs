//! docs/specs/filters-agg AC-5: a `filters` reduces across shards through
//! the coordinator's `reduce_shards`.

use oss_coordinator::reduce::aggs::reduce_shards;
use oss_reader::aggs::bucket::filters::FiltersResult;
use oss_reader::aggs::partial::Value;

fn shard(counts: [u64; 2]) -> Vec<(String, Value)> {
    vec![(
        "f".to_string(),
        Value::Filters(Box::new(FiltersResult {
            keyed: true,
            buckets: vec![
                ("a".to_string(), counts[0]),
                ("_other_".to_string(), counts[1]),
            ],
        })),
    )]
}

#[test]
fn a_filters_adds_its_counts_over_the_shards_that_answered() {
    let out = reduce_shards(&[shard([1, 2]), shard([30, 40])]).expect("reduces");
    let [(name, Value::Filters(f))] = out.as_slice() else {
        panic!("{out:?}");
    };
    assert_eq!(name, "f");
    assert!(f.keyed);
    assert_eq!(
        f.buckets,
        [("a".to_string(), 31), ("_other_".to_string(), 42)]
    );
}
