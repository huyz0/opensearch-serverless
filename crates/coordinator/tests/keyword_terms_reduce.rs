//! docs/specs/keyword-terms-agg AC-3: a keyword `terms` reduces across
//! shards through the coordinator's `reduce_shards`.

use oss_coordinator::reduce::aggs::reduce_shards;
use oss_reader::aggs::bucket::keyword_terms::KeywordTermsResult;
use oss_reader::aggs::bucket::terms::{TermsOrder, TermsSpec};
use oss_reader::aggs::partial::{Format, Value};

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
            sub_meta: Vec::new(),
            subs: Vec::new(),
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

// docs/specs/keyword-terms-agg AC-11
#[test]
fn sub_aggregations_reduce_with_their_buckets_across_shards() {
    let with_subs = |buckets: &[(&str, u64, f64, u64)]| {
        let mut parts = shard(0, &[]);
        let Some((_, Value::KeywordTerms(t))) = parts.first_mut() else {
            panic!("not a terms");
        };
        t.sub_meta = vec![("m".to_owned(), Format::Raw)];
        t.buckets = buckets.iter().map(|b| (b.0.to_owned(), b.1)).collect();
        t.subs = buckets
            .iter()
            .map(|b| {
                vec![Value::Avg {
                    sum: b.2,
                    count: b.3,
                }]
            })
            .collect();
        parts
    };
    let out = reduce_shards(&[
        with_subs(&[("a", 5, 50.0, 5), ("b", 3, 9.0, 3)]),
        with_subs(&[("b", 4, 40.0, 4), ("c", 1, 7.0, 1)]),
    ])
    .expect("reduces");
    let [(_, Value::KeywordTerms(t))] = out.as_slice() else {
        panic!("{out:?}");
    };
    assert_eq!(t.buckets, [("b".to_owned(), 7), ("a".to_owned(), 5)]);
    assert_eq!(
        t.subs,
        [
            vec![Value::Avg {
                sum: 49.0,
                count: 7
            }],
            vec![Value::Avg {
                sum: 50.0,
                count: 5
            }]
        ]
    );
}
