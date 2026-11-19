//! docs/specs/terms-agg AC-5: a `terms` reduces across shards through the
//! coordinator's `reduce_shards`.

use oss_coordinator::reduce::aggs::reduce_shards;
use oss_reader::aggs::bucket::terms::{TermsOrder, TermsResult, TermsSpec};
use oss_reader::aggs::metric::partial::Value;

fn shard(other: u64, buckets: &[(i64, u64)]) -> Vec<(String, Value)> {
    vec![(
        "t".to_string(),
        Value::Terms(Box::new(TermsResult {
            spec: TermsSpec {
                size: 2,
                shard_size: 13,
                min_doc_count: 1,
                order: TermsOrder::CountDesc,
            },
            other_doc_count: other,
            doc_count_error: 0,
            buckets: buckets.to_vec(),
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
