//! docs/specs/terms-agg AC-5: a `terms` reduces across shards through the
//! coordinator's `reduce_shards`.

use oss_coordinator::reduce::aggs::reduce_shards;
use oss_reader::aggs::bucket::terms::{TermsOrder, TermsResult, TermsSpec};
use oss_reader::aggs::partial::Value;

fn shard(other: u64, buckets: &[(i64, u64)]) -> Vec<(String, Value)> {
    vec![(
        "t".to_string(),
        Value::Terms(Box::new(TermsResult {
            spec: TermsSpec {
                size: 2,
                shard_size: 13,
                min_doc_count: 1,
                order: TermsOrder::CountDesc,
                keyword: Default::default(),
                show_error: false,
                shard_min_doc_count: 0,
            },
            other_doc_count: other,
            doc_count_error: 0,
            buckets: buckets.to_vec(),
            ..TermsResult::default()
        })),
    )]
}

#[test]
fn a_terms_reduces_over_the_shards_that_answered() {
    let out = reduce_shards(&[
        shard(0, &[(1, 5), (2, 3), (3, 1)]),
        shard(1, &[(2, 4), (3, 4), (4, 2)]),
    ])
    .expect("reduces");
    let [(name, Value::Terms(t))] = out.as_slice() else {
        panic!("{out:?}");
    };
    assert_eq!(name, "t");
    assert_eq!(t.buckets, [(2, 7), (1, 5)]);
    assert_eq!(t.other_doc_count, 8);
    assert_eq!(t.doc_count_error, 0);
}

#[test]
fn a_bucket_carries_the_error_of_the_shards_that_lack_it() {
    let shown = |buckets: &[(i64, u64)]| {
        let mut parts = shard(0, buckets);
        let Value::Terms(t) = &mut parts[0].1 else {
            unreachable!()
        };
        t.spec.shard_size = 2;
        t.spec.show_error = true;
        parts
    };
    let out =
        reduce_shards(&[shown(&[(1, 10), (2, 4)]), shown(&[(2, 6), (3, 5)])]).expect("reduces");
    let [(_, Value::Terms(t))] = out.as_slice() else {
        panic!("{out:?}");
    };
    assert_eq!(t.buckets, [(1, 10), (2, 10)]);
    assert_eq!(t.bucket_errors, [5, 0]);
    assert_eq!(t.doc_count_error, 9);
}
