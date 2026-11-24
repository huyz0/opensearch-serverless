//! A keyword `terms` ordered by a metric sub-aggregation, run on the index
//! of four segments.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::keyword_collect_tests::{keys, shard, terms};
use super::keyword_subs_tests::{DOCS, inner};
use crate::aggs::partial::Value;

fn failure_of(shard: &crate::OpenShard<oss_blob::Emulator>, body: &str) -> String {
    match crate::dsl::parse_chunks([body.as_bytes()]) {
        Err(e) => format!("{:?}", e.into_failure()),
        Ok(s) => format!(
            "{:?}",
            crate::search::search(shard, "idx", &s, &lucene_analysis::Analyzer::standard(None))
                .expect_err("refused")
        ),
    }
}

// AC-17
#[test]
fn a_terms_orders_by_a_metric_sub_aggregation() {
    let shard = shard("kw-order-sub", DOCS);
    // The largest `n` of tag `t<k>` is 199 - ((199 - k) % 16 ... ) as
    // computed: t7 199, t6 198, t5 197, then t4 196, ..., t8 184, t9 185.
    let largest = |k: usize| (0..200).filter(|i| i % 16 == k).max().unwrap();
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","size":3,"order":{"hi":"desc"}},
            "aggs":{"hi":{"max":{"field":"n"}}}}}}"#,
    );
    // The shard keeps `shard_size` 14 of 16 by the metric: t8 and t9 go.
    assert_eq!(got.buckets.len(), 14);
    assert_eq!(got.other_doc_count, 24);
    let held: Vec<&str> = got.buckets.iter().map(|b| b.0.as_str()).collect();
    assert!(!held.contains(&"t8") && !held.contains(&"t9"), "{held:?}");
    // Each kept bucket carries its own metric, in the order listed.
    for (at, (term, _)) in got.buckets.iter().enumerate() {
        let k: usize = term[1..].parse().unwrap();
        assert_eq!(got.subs[at], [Value::Max(largest(k) as f64)], "{term}");
    }
    let reduced = crate::aggs::bucket::keyword_terms::reduce_results(&[&got]);
    assert_eq!(keys(&reduced), [("t7", 13), ("t6", 13), ("t5", 13)]);
    assert_eq!(reduced.subs[0], [Value::Max(largest(7) as f64)]);
    // Ascending, through the `value` key and through a stats key.
    for order in [r#"{"hi.value":"asc"}"#, r#"{"st.max":"asc"}"#] {
        let got = terms(
            &shard,
            &format!(
                r#"{{"aggs":{{"t":{{"terms":{{"field":"tag","size":2,"order":{order}}},
                "aggs":{{"hi":{{"max":{{"field":"n"}}}},"st":{{"stats":{{"field":"n"}}}}}}}}}}}}"#
            ),
        );
        let reduced = crate::aggs::bucket::keyword_terms::reduce_results(&[&got]);
        assert_eq!(keys(&reduced), [("t8", 12), ("t9", 12)], "{order}");
        // The shard cut the largest metrics, t7 and t6.
        let held: Vec<&str> = got.buckets.iter().map(|b| b.0.as_str()).collect();
        assert!(!held.contains(&"t7") && !held.contains(&"t6"), "{held:?}");
    }
}

#[test]
fn nan_metrics_sort_last_and_ties_list_by_term() {
    let shard = shard("kw-order-nan", DOCS);
    // Only n below 5 match: t0 to t4 have an average, t5 to t7 are empty.
    let by = |direction: &str| {
        let got = terms(
            &shard,
            &format!(
                r#"{{"query":{{"range":{{"n":{{"lt":5}}}}}},"aggs":{{"t":{{"terms":{{
                "field":"tag","size":8,"min_doc_count":0,"include":["t0","t1","t2","t3","t4","t5","t6","t7"],
                "order":{{"a":"{direction}"}}}},"aggs":{{"a":{{"avg":{{"field":"n"}}}}}}}}}}}}"#
            ),
        );
        crate::aggs::bucket::keyword_terms::reduce_results(&[&got])
            .buckets
            .into_iter()
            .map(|b| b.0)
            .collect::<Vec<_>>()
    };
    assert_eq!(by("desc"), ["t4", "t3", "t2", "t1", "t0", "t5", "t6", "t7"]);
    assert_eq!(by("asc"), ["t0", "t1", "t2", "t3", "t4", "t5", "t6", "t7"]);
    // Equal metrics list by term: every value count is 12 or 13.
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","size":3,"order":{"c":"desc"}},
            "aggs":{"c":{"value_count":{"field":"n"}}}}}}"#,
    );
    let reduced = crate::aggs::bucket::keyword_terms::reduce_results(&[&got]);
    assert_eq!(keys(&reduced), [("t0", 13), ("t1", 13), ("t2", 13)]);
}

#[test]
fn an_inner_terms_orders_within_each_outer_bucket() {
    let shard = shard("kw-order-inner", DOCS);
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"tag","include":["t3"]},"aggs":{
            "k":{"terms":{"field":"k0","size":2,"order":{"lo":"asc"}},
            "aggs":{"lo":{"min":{"field":"n"}}}}}}}}"#,
    );
    let k = inner(&got, "t3", 0);
    let reduced = crate::aggs::bucket::keyword_terms::reduce_results(&[&k]);
    // t3's documents 3, 19, 35, ...: the smallest `n` of each `k0` value is
    // the first of them with that `i % 7`: v3 at 3, v5 at 19, v0 at 35.
    assert_eq!(
        reduced
            .buckets
            .iter()
            .map(|b| b.0.as_str())
            .collect::<Vec<_>>(),
        ["v3", "v5"]
    );
    assert_eq!(reduced.subs[0], [Value::Min(3.0)]);
}

// AC-18
#[test]
fn bad_order_paths_fail_as_opensearch_fails_them() {
    let shard = shard("kw-order-bad", DOCS);
    let with = |order: &str, field: &str| {
        failure_of(
            &shard,
            &format!(
                r#"{{"aggs":{{"t":{{"terms":{{"field":"{field}","order":{order}}},
                "aggs":{{"hi":{{"max":{{"field":"n"}}}}}}}}}}}}"#
            ),
        )
    };
    let e = with(r#"{"zz":"asc"}"#, "tag");
    assert!(e.contains("aggregation_execution_exception"), "{e}");
    assert!(e.contains("status: 500"), "{e}");
    assert!(
        e.contains("Invalid aggregation order path [zz]. The provided aggregation [zz]"),
        "{e}"
    );
    let e = with(r#"{"hi.max":"asc"}"#, "tag");
    assert!(
        e.contains("Ordering on a single-value metrics aggregation"),
        "{e}"
    );
    // An unmapped field is checked too.
    let e = with(r#"{"zz":"asc"}"#, "nope");
    assert!(e.contains("Invalid aggregation order path [zz]"), "{e}");
    // A malformed element fails at parse.
    let e = with(r#"{"hi[":"asc"}"#, "tag");
    assert!(
        e.contains("Invalid path element [hi[] in path [hi[]"),
        "{e}"
    );
}

// AC-19
#[test]
fn an_order_by_a_sub_aggregation_reserves_every_term() {
    use crate::aggs::bucket::keyword_collect::{BUCKET_BYTES, KEY_COPIES, Parent, reserved_bytes};
    let shard = shard("kw-order-bytes", DOCS);
    let spec = crate::aggs::bucket::terms::TermsSpec {
        size: 1,
        shard_size: 1,
        sub_order: Some(crate::aggs::bucket::sub_order::SubOrder {
            path: "m".to_owned(),
            ascending: true,
        }),
        ..Default::default()
    };
    let plain = crate::aggs::bucket::terms::TermsSpec {
        sub_order: None,
        ..spec.clone()
    };
    // 64 terms of up to three bytes are all kept; one is kept otherwise.
    assert_eq!(
        reserved_bytes(&shard, "tag", &spec, 1, Parent::TOP)
            - reserved_bytes(&shard, "tag", &plain, 1, Parent::TOP),
        63 * (3 * KEY_COPIES + BUCKET_BYTES)
            + 63 * crate::aggs::bucket::keyword_collect::SUB_COPIES * std::mem::size_of::<Value>()
    );
}

#[test]
fn an_order_by_a_sub_aggregation_stops_at_the_bucket_limit() {
    use crate::aggs::bucket::BUCKET_LIMIT;
    use crate::aggs::bucket::keyword_collect::{BUCKET_BYTES, KEY_COPIES, Parent, reserved_bytes};
    let shard = shard("kw-order-limit", DOCS);
    let spec = crate::aggs::bucket::terms::TermsSpec {
        sub_order: Some(crate::aggs::bucket::sub_order::SubOrder {
            path: "m".to_owned(),
            ascending: true,
        }),
        ..Default::default()
    };
    let plain = crate::aggs::bucket::terms::TermsSpec {
        sub_order: None,
        shard_size: 1,
        ..spec.clone()
    };
    // Under 5,000 parent buckets keeping every one of 64 terms the kept
    // buckets would be 320,000; they stop at the limit.
    let parent = Parent {
        cells: 1,
        kept: 5_000,
    };
    let grown = reserved_bytes(&shard, "tag", &spec, 0, parent)
        - reserved_bytes(&shard, "tag", &plain, 0, parent);
    // `plain` keeps 5,000 and `spec` the limit, each bucket 3 key copies of 3 bytes.
    assert_eq!(
        grown,
        (BUCKET_LIMIT - 5_000) * (3 * KEY_COPIES + BUCKET_BYTES)
    );
}
