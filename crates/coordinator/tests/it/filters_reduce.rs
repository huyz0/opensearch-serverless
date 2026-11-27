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
            single: false,
            buckets: vec![
                ("a".to_string(), counts[0]),
                ("_other_".to_string(), counts[1]),
            ],
            ..FiltersResult::default()
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

#[test]
fn a_filters_reduces_each_buckets_sub_aggregations_over_the_shards() {
    use oss_reader::aggs::partial::Format;
    let with_subs = |counts: [u64; 2], mins: [f64; 2]| {
        let mut parts = shard(counts);
        let Value::Filters(f) = &mut parts[0].1 else {
            panic!("not a filters");
        };
        f.sub_meta = vec![("m".to_string(), Format::Raw)];
        f.subs = mins.iter().map(|m| vec![Value::Min(*m)]).collect();
        parts
    };
    let out = reduce_shards(&[with_subs([1, 2], [5.0, 7.0]), with_subs([3, 4], [6.0, 2.0])])
        .expect("reduces");
    let [(_, Value::Filters(f))] = out.as_slice() else {
        panic!("{out:?}");
    };
    assert_eq!(
        f.subs,
        [vec![Value::Min(5.0)], vec![Value::Min(2.0)]],
        "the smaller minimum of each bucket"
    );
}
