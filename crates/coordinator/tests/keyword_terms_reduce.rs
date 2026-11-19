//! docs/specs/keyword-terms-agg AC-3: a keyword `terms` reduces across
//! shards through the coordinator's `reduce_shards`.

use oss_coordinator::reduce::aggs::reduce_shards;
use oss_reader::aggs::bucket::keyword_terms::KeywordTermsResult;
use oss_reader::aggs::bucket::terms::{TermsOrder, TermsSpec};
use oss_reader::aggs::partial::Value;

fn shard(other: u64, buckets: &[(&str, u64)]) -> Vec<(String, Value)> {
    vec![(
        "t".to_string(),
        Value::KeywordTerms(Box::new(KeywordTermsResult {
            spec: TermsSpec {
                size: 2,
                shard_size: 13,
                min_doc_count: 1,
                order: TermsOrder::CountDesc,
                keyword: Default::default(),
            },
            other_doc_count: other,
            doc_count_error: 0,
            buckets: buckets.iter().map(|&(k, c)| (k.to_owned(), c)).collect(),
        })),
    )]
}

#[test]
fn a_keyword_terms_reduces_over_the_shards_that_answered() {
    let out = reduce_shards(&[
        shard(0, &[("a", 5), ("b", 3), ("c", 1)]),
        shard(1, &[("b", 4), ("c", 4), ("d", 2)]),
    ])
    .expect("reduces");
    let [(name, Value::KeywordTerms(t))] = out.as_slice() else {
        panic!("{out:?}");
    };
    assert_eq!(name, "t");
    assert_eq!(t.buckets, [("b".to_owned(), 7), ("a".to_owned(), 5)]);
    assert_eq!(t.other_doc_count, 8);
    assert_eq!(t.doc_count_error, 0);
}
