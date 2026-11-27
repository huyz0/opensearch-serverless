//! `significant_terms` parsed, scored, reduced and run.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use crate::aggs::bucket::significant::{
    Heuristic, SigBucket, SignificantResult, SignificantSpec, reduce, write_significant,
};
use crate::aggs::metric::Metric;
use crate::aggs::partial::{Partial, Value};
use crate::dsl::parse_chunks;
use crate::search::{ShardError, search};
use crate::{OpenShard, open_shard};

fn reason(body: &str) -> String {
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    let mut text = failure.reason.clone();
    if let Some(cause) = &failure.cause {
        text.push_str(" | ");
        text.push_str(&cause.reason);
    }
    text
}

fn spec_of(body: &str) -> SignificantSpec {
    let agg = parse_chunks([body.as_bytes()])
        .expect("parses")
        .aggs
        .remove(0);
    assert_eq!(agg.metric, Metric::SignificantTerms);
    *agg.significant.expect("a spec")
}

// AC-1
#[test]
fn the_body_parses_with_its_defaults_and_options() {
    let spec = spec_of(r#"{"aggs":{"s":{"significant_terms":{"field":"tag"}}}}"#);
    assert_eq!(spec.size, 10);
    assert_eq!(spec.min_doc_count, 3);
    assert_eq!(spec.shard_size, 50);
    assert_eq!(spec.heuristic, Heuristic::Jlh);
    let spec = spec_of(
        r#"{"aggs":{"s":{"significant_terms":{"field":"tag","size":2,"min_doc_count":1,
            "shard_min_doc_count":9,"percentage":{}}}}}"#,
    );
    assert_eq!((spec.size, spec.min_doc_count), (2, 1));
    assert_eq!(spec.shard_size, 26);
    assert_eq!(spec.shard_min_doc_count, 1);
    assert_eq!(spec.heuristic, Heuristic::Percentage);
    assert_eq!(
        Metric::from_name("significant_terms"),
        Some(Metric::SignificantTerms)
    );
}

// AC-1
#[test]
fn bad_and_unserved_options_fail() {
    let got = reason(r#"{"aggs":{"s":{"significant_terms":{"field":"tag","size":0}}}}"#);
    assert!(
        got.contains("[size] must be greater than 0. Found [0] in [s]"),
        "{got}"
    );
    let got = reason(r#"{"aggs":{"s":{"significant_terms":{"field":"tag","min_doc_count":-1}}}}"#);
    assert!(
        got.contains("[minDocCount] must be greater than or equal to 0"),
        "{got}"
    );
    let got = reason(r#"{"aggs":{"s":{"significant_terms":{"field":"tag","sizes":1}}}}"#);
    assert!(got.contains("unknown field [sizes]"), "{got}");
    let got = reason(r#"{"aggs":{"s":{"significant_terms":{"size":1}}}}"#);
    assert!(
        got.contains("Required one of fields [field, script]"),
        "{got}"
    );
    for key in [
        "background_filter",
        "include",
        "exclude",
        "gnd",
        "chi_square",
        "missing",
    ] {
        let body =
            format!(r#"{{"aggs":{{"s":{{"significant_terms":{{"field":"tag","{key}":{{}}}}}}}}}}"#);
        let got = reason(&body);
        assert!(
            got.contains("not supported by this node yet"),
            "{key}: {got}"
        );
    }
}

// AC-5
#[test]
fn sub_aggregations_and_nesting_are_refused() {
    let got = reason(
        r#"{"aggs":{"s":{"significant_terms":{"field":"tag"},"aggs":{"m":{"max":{"field":"n"}}}}}}"#,
    );
    assert!(got.contains("sub-aggregation"), "{got}");
    let got = reason(
        r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"s":{"significant_terms":{"field":"tag"}}}}}}"#,
    );
    assert!(got.contains("sub-aggregation"), "{got}");
}

// AC-2
#[test]
fn the_heuristics_score_as_java_does() {
    let jlh = Heuristic::Jlh.score(2, 20, 13, 200);
    assert!((jlh - 0.035 * (0.1 / 0.065)).abs() < 1e-12, "{jlh}");
    // A background count of 0 reads as 1.
    let lone = Heuristic::Jlh.score(1, 10, 0, 100);
    assert!((lone - (0.1 - 0.01) * 10.0).abs() < 1e-12, "{lone}");
    assert_eq!(Heuristic::Jlh.score(1, 20, 12, 200), 0.0);
    assert_eq!(Heuristic::Jlh.score(1, 0, 12, 200), 0.0);
    assert_eq!(Heuristic::Percentage.score(2, 20, 8, 200), 0.25);
    assert_eq!(Heuristic::Percentage.score(2, 20, 0, 200), 0.0);
}

fn result(spec: &SignificantSpec, sizes: (u64, u64), buckets: &[(&str, u64, u64)]) -> Value {
    Value::Significant(Box::new(SignificantResult {
        spec: spec.clone(),
        subset_size: sizes.0,
        superset_size: sizes.1,
        buckets: buckets
            .iter()
            .map(|(t, s, b)| SigBucket {
                term: (*t).to_owned(),
                subset_df: *s,
                superset_df: *b,
            })
            .collect(),
    }))
}

// AC-3
#[test]
fn a_shard_keeps_the_best_size_with_enough_documents() {
    let spec = SignificantSpec {
        size: 2,
        min_doc_count: 2,
        ..SignificantSpec::default()
    };
    let mut r = SignificantResult {
        spec,
        subset_size: 20,
        superset_size: 200,
        buckets: [("a", 2, 100), ("b", 5, 10), ("c", 1, 1), ("d", 4, 8)]
            .iter()
            .map(|(t, s, b)| SigBucket {
                term: (*t).to_owned(),
                subset_df: *s,
                superset_df: *b,
            })
            .collect(),
    };
    r.select();
    let terms: Vec<&str> = r.buckets.iter().map(|b| b.term.as_str()).collect();
    assert_eq!(terms, ["b", "d"]);
}

// AC-3
#[test]
fn a_reduce_sums_shards_rescores_and_drops_scores_of_zero() {
    let spec = SignificantSpec {
        size: 5,
        min_doc_count: 1,
        ..SignificantSpec::default()
    };
    let a = result(&spec, (10, 100), &[("x", 3, 5), ("y", 1, 50)]);
    let b = result(&spec, (10, 100), &[("x", 2, 5), ("z", 1, 2)]);
    let Some(Value::Significant(r)) = reduce(&[a, b]) else {
        panic!("not reduced");
    };
    assert_eq!((r.subset_size, r.superset_size), (20, 200));
    let got: Vec<(&str, u64, u64)> = r
        .buckets
        .iter()
        .map(|b| (b.term.as_str(), b.subset_df, b.superset_df))
        .collect();
    // `y`: 1/20 against 50/200 scores 0 and is dropped.
    assert_eq!(got, [("x", 5, 10), ("z", 1, 2)]);
    let mut out = Vec::new();
    write_significant(&mut out, &r).expect("writes");
    let text = String::from_utf8(out).expect("utf8");
    assert!(
        text.starts_with("\"doc_count\":20,\"bg_count\":200,\"buckets\":[{\"key\":\"x\""),
        "{text}"
    );
    assert!(text.contains("\"doc_count\":5,\"score\":"), "{text}");
    assert!(text.contains("\"bg_count\":10}"), "{text}");
}

fn shard(name: &str) -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", &Spec::new(name, 200).segments_of(50).with_texts(1)) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", name, 0).expect("opens")
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Vec<Partial>, ShardError> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
}

// AC-4
#[test]
fn a_query_makes_its_over_represented_terms_significant() {
    let shard = shard("significant-run");
    // Documents 0 to 19: tags t0 to t3 twice, the others once.
    let parts = run(
        &shard,
        r#"{"size":0,"query":{"range":{"n":{"lt":20}}},"aggs":{
            "s":{"significant_terms":{"field":"tag","min_doc_count":1}}}}"#,
    )
    .expect("searches");
    let Value::Significant(r) = &parts[0].value else {
        panic!("not significant");
    };
    assert_eq!((r.subset_size, r.superset_size), (20, 200));
    // A shard keeps the best `size` whatever their score, as OpenSearch does.
    assert_eq!(r.buckets.len(), 10);
    let Some(Value::Significant(reduced)) = reduce(&[parts[0].value.clone()]) else {
        panic!("not reduced");
    };
    let got: Vec<(&str, u64, u64)> = reduced
        .buckets
        .iter()
        .map(|b| (b.term.as_str(), b.subset_df, b.superset_df))
        .collect();
    assert_eq!(
        got,
        [("t0", 2, 13), ("t1", 2, 13), ("t2", 2, 13), ("t3", 2, 13)]
    );
}

// AC-4
#[test]
fn the_default_minimum_and_a_query_of_everything_leave_no_bucket() {
    let shard = shard("significant-none");
    let parts = run(
        &shard,
        r#"{"size":0,"query":{"range":{"n":{"lt":20}}},"aggs":{
            "s":{"significant_terms":{"field":"tag"}}}}"#,
    )
    .expect("searches");
    let Value::Significant(r) = &parts[0].value else {
        panic!("not significant");
    };
    assert!(r.buckets.is_empty());
    let parts = run(
        &shard,
        r#"{"size":0,"aggs":{"s":{"significant_terms":{"field":"tag","min_doc_count":1}}}}"#,
    )
    .expect("searches");
    let Some(Value::Significant(r)) = reduce(&[parts[0].value.clone()]) else {
        panic!("not reduced");
    };
    assert_eq!(r.subset_size, 200);
    assert!(r.buckets.is_empty());
}

// AC-5
#[test]
fn an_unmapped_field_answers_nothing_and_a_numeric_one_is_refused() {
    let shard = shard("significant-field");
    let parts = run(
        &shard,
        r#"{"size":0,"aggs":{"s":{"significant_terms":{"field":"nope"}}}}"#,
    )
    .expect("searches");
    let Value::Significant(r) = &parts[0].value else {
        panic!("not significant");
    };
    assert_eq!((r.subset_size, r.superset_size, r.buckets.len()), (0, 0, 0));
    let e = run(
        &shard,
        r#"{"size":0,"aggs":{"s":{"significant_terms":{"field":"n"}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(e.status, 400);
    assert!(e.failure.reason.contains("not supported by this node yet"));
}
