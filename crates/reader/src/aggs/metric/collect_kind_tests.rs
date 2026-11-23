//! `value_from` refuses an engine result of another kind than its aggregation.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

#[cfg(test)]
use crate::dsl::parse_chunks;

#[test]
fn a_result_of_another_kind_than_the_aggregation_is_an_engine_error() {
    use lucene_aggs::bucket_aggs::AggResult;

    use super::collect::value_from;
    let body = r#"{"aggs":{"a":{"sum":{"field":"n"}}}}"#;
    let aggs = parse_chunks([body.as_bytes()]).expect("parses").aggs;
    let wrong = [
        AggResult::NumericTerms {
            buckets: vec![(0, vec![(1, 1)])],
            subs: Vec::new(),
        },
        AggResult::Histogram {
            buckets: vec![vec![(1.0, 1)]],
            subs: Vec::new(),
        },
        AggResult::DateHistogram {
            buckets: vec![vec![(1, 1)]],
            subs: Vec::new(),
        },
    ];
    for result in wrong {
        let err = value_from(&aggs[0], Some(result.clone())).expect_err("refused");
        assert_eq!(err.status, 500, "{result:?}");
    }
}
