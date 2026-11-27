//! `multi_terms` parsed, reduced, written and run.
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

use crate::aggs::bucket::multi::{MultiKey, MultiPart, MultiResult, reduce, write_multi};
use crate::aggs::bucket::terms::{TermsOrder, TermsSpec};
use crate::aggs::metric::Metric;
use crate::aggs::partial::{Partial, Value};
use crate::dsl::parse_chunks;
use crate::search::{ShardError, search};
use crate::{OpenShard, open_shard};

const RECORDED: &str =
    include_str!("../../../../../fixtures/multi-terms/java-multi-terms-3.10.0.txt");

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

fn spec_of(body: &str) -> (Vec<String>, TermsSpec) {
    let agg = parse_chunks([body.as_bytes()])
        .expect("parses")
        .aggs
        .remove(0);
    assert_eq!(agg.metric, Metric::MultiTerms);
    let fields = agg
        .composite
        .expect("sources")
        .sources
        .iter()
        .map(|s| s.field.clone())
        .collect();
    (fields, *agg.terms.expect("a spec"))
}

// AC-1
#[test]
fn the_body_parses_with_its_defaults_and_options() {
    let (fields, spec) =
        spec_of(r#"{"aggs":{"m":{"multi_terms":{"terms":[{"field":"tag"},{"field":"n"}]}}}}"#);
    assert_eq!(fields, ["tag", "n"]);
    assert_eq!(
        (
            spec.size,
            spec.shard_size,
            spec.min_doc_count,
            spec.shard_min_doc_count
        ),
        (10, 25, 1, 0)
    );
    assert_eq!(spec.order, TermsOrder::CountDesc);
    let (_, spec) = spec_of(
        r#"{"aggs":{"m":{"multi_terms":{"terms":[{"field":"tag"},{"field":"n"}],
        "size":3,"shard_size":"5","min_doc_count":2,
        "order":{"_key":"desc"}}}}}"#,
    );
    assert_eq!((spec.size, spec.shard_size, spec.min_doc_count), (3, 5, 2));
    assert_eq!(spec.order, TermsOrder::KeyDesc);
}

// AC-1, AC-6
#[test]
fn bad_and_unserved_options_fail_as_java_fails() {
    let two = r#""terms":[{"field":"tag"},{"field":"n"}]"#;
    for (body, want) in [
        (
            r#""terms":[{"field":"tag"}]"#.to_owned(),
            "multi term aggregation must has at least 2 terms. Found [1] in [m]",
        ),
        (
            format!(r#"{two},"size":0"#),
            "[size] must be greater than 0. Found [0] in [m]",
        ),
        (
            format!(r#"{two},"shard_size":-2"#),
            "[shardSize] must be greater than 0. Found [-2] in [m]",
        ),
        (
            format!(r#"{two},"min_doc_count":-1"#),
            "[minDocCount] must be greater than or equal to 0. Found [-1] in [m]",
        ),
        (
            format!(r#"{two},"shard_min_doc_count":-1"#),
            "[shardMinDocCount] must be greater than or equal to 0. Found [-1] in [m]",
        ),
        (
            r#""size":3"#.to_owned(),
            "[terms] must not be null. Found null terms in [m]",
        ),
        (
            format!(r#"{two},"nope":1"#),
            "[multi_terms] unknown field [nope]",
        ),
        (
            r#""terms":[{"field":"tag"},{"nope":1}]"#.to_owned(),
            "[term] unknown field [nope]",
        ),
        (
            r#""terms":[{"field":"tag"},{"missing":"x"}]"#.to_owned(),
            "not supported by this node yet",
        ),
        (
            r#""terms":[{"field":"tag"},{"field":"n","include":"a.*"}]"#.to_owned(),
            "not supported by this node yet",
        ),
        (
            format!(r#"{two},"show_term_doc_count_error":true"#),
            "not supported by this node yet",
        ),
        (
            format!(r#"{two},"collect_mode":"breadth_first""#),
            "not supported by this node yet",
        ),
        (
            format!(r#"{two},"order":{{"sub":"desc"}}"#),
            "not supported by this node yet",
        ),
        (
            format!(r#"{two},"order":[{{"_count":"desc"}}]"#),
            "doesn't support values of type",
        ),
        (
            format!(r#"{two},"order":{{"_key":"asc"}},"shard_min_doc_count":1"#),
            "not supported by this node yet",
        ),
    ] {
        let got = reason(&format!(
            r#"{{"aggs":{{"m":{{"multi_terms":{{{body}}}}}}}}}"#
        ));
        assert!(got.contains(want), "{body}: {got}");
    }
}

// AC-6
#[test]
fn sub_aggregations_and_nesting_are_refused() {
    let got = reason(
        r#"{"aggs":{"m":{"multi_terms":{"terms":[{"field":"tag"},{"field":"n"}]},
        "aggs":{"x":{"max":{"field":"n"}}}}}}"#,
    );
    assert!(got.contains("sub-aggregation"), "{got}");
    let got = reason(
        r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"m":{"multi_terms":
        {"terms":[{"field":"tag"},{"field":"n"}]}}}}}}"#,
    );
    assert!(got.contains("sub-aggregation"), "{got}");
}

fn key(text: &str, n: i64) -> MultiKey {
    vec![MultiPart::Text(text.to_owned()), MultiPart::Long(n)]
}

/// A shard of the recorder: ordered by count descending, `other` documents
/// in buckets it did not keep.
fn shard_of(
    buckets: &[(&str, i64, u64)],
    size: usize,
    shard_size: usize,
    other: u64,
) -> MultiResult {
    let spec = TermsSpec {
        size,
        shard_size,
        ..TermsSpec::default()
    };
    let counted = buckets.iter().map(|(t, n, c)| (key(t, *n), *c)).collect();
    MultiResult {
        other_doc_count: other,
        ..MultiResult::of_shard(&spec, counted)
    }
}

fn recorded(name: &str) -> &'static str {
    RECORDED
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
        .unwrap_or_else(|| panic!("no fixture {name}"))
}

/// The recorder's whole response for the final reduce of `shards`.
fn written(shards: &[MultiResult]) -> String {
    let parts: Vec<Value> = shards
        .iter()
        .map(|s| Value::Multi(Box::new(s.clone())))
        .collect();
    let Some(Value::Multi(reduced)) = reduce(&parts) else {
        panic!("not reduced");
    };
    let mut out = Vec::new();
    write_multi(&mut out, &reduced).expect("writes");
    format!(r#"{{"m":{{{}}}}}"#, String::from_utf8(out).expect("utf-8"))
}

// AC-2
#[test]
fn a_reduce_is_written_as_the_bytes_java_wrote() {
    let a = || shard_of(&[("a", 1, 3), ("b", 2, 1)], 1, 2, 4);
    assert_eq!(written(&[a()]), recorded("one"));
    let c = shard_of(&[("a", 1, 2), ("c", 3, 2)], 1, 2, 0);
    assert_eq!(written(&[a(), c]), recorded("two"));
    let a = shard_of(&[("a", 1, 3), ("b", 2, 1)], 2, 2, 4);
    let c = shard_of(&[("c", 3, 2), ("d", 4, 1)], 2, 2, 0);
    assert_eq!(written(&[a, c]), recorded("three"));
    let a = shard_of(&[("a", 1, 3), ("b", 2, 1)], 10, 20, 0);
    let c = shard_of(&[("a", 1, 2), ("c", 3, 2)], 10, 20, 0);
    assert_eq!(written(&[a, c]), recorded("merged"));
}

// AC-3
#[test]
fn equal_counts_order_by_key_term_by_term() {
    let one = |buckets: &[(&str, i64, u64)]| written(&[shard_of(buckets, 10, 20, 0)]);
    assert_eq!(one(&[("b", 1, 5), ("a", 1, 5)]), recorded("ties_text"));
    assert_eq!(
        one(&[("a", 10, 5), ("a", 9, 5), ("a", 100, 5)]),
        recorded("ties_number")
    );
    assert_eq!(
        one(&[("a", 1, 3), ("b", 2, 3), ("a", 0, 1)]),
        recorded("ties_mixed")
    );
}

#[test]
fn a_reduce_of_nothing_or_of_another_kind_is_none() {
    assert_eq!(reduce(&[]), None);
    assert_eq!(reduce(&[Value::Min(1.0)]), None);
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

fn multi(parts: &[Partial]) -> &MultiResult {
    let Value::Multi(m) = &parts[0].value else {
        panic!("not multi_terms");
    };
    m
}

// AC-4, AC-5
#[test]
fn a_shard_counts_the_combinations_and_keeps_the_best_by_the_order() {
    let shard = shard("multi-run");
    // Documents 0 to 19: the tags t0 to t3 twice, each with two numbers.
    let body = |options: &str| {
        format!(
            r#"{{"size":0,"query":{{"range":{{"n":{{"lt":20}}}}}},"aggs":{{
            "m":{{"multi_terms":{{"terms":[{{"field":"tag"}},{{"field":"n"}}]{options}}}}}}}}}"#
        )
    };
    let parts = run(&shard, &body("")).expect("searches");
    let m = multi(&parts);
    assert_eq!(m.buckets.len(), 20);
    assert!(m.buckets.iter().all(|(_, c)| *c == 1));
    assert_eq!(m.other_doc_count, 0);
    // Equal counts by key: the tag's bytes, then the number.
    assert_eq!(m.buckets[0].0, key("t0", 0));
    assert_eq!(m.buckets[1].0, key("t0", 16));
    assert_eq!(m.buckets[2].0, key("t1", 1));
    // A shard keeps `shard_size`, and the rest are other documents.
    let parts = run(&shard, &body(r#","size":2,"shard_size":5"#)).expect("searches");
    let m = multi(&parts);
    assert_eq!((m.buckets.len(), m.other_doc_count), (5, 15));
    // Ordered by key descending.
    let parts = run(&shard, &body(r#","order":{"_key":"desc"},"size":2"#)).expect("searches");
    let m = multi(&parts);
    assert_eq!(m.buckets[0].0, key("t9", 9));
    // The response of a single shard cuts at `size`.
    let mut out = Vec::new();
    write_multi(&mut out, m).expect("writes");
    let text = String::from_utf8(out).expect("utf-8");
    assert!(
        text.starts_with(r#""doc_count_error_upper_bound":0,"sum_other_doc_count":18,"buckets":[{"key":["t9",9],"key_as_string":"t9|9","doc_count":1},"#),
        "{text}"
    );
}

// AC-4
#[test]
fn an_unmapped_term_answers_nothing() {
    let shard = shard("multi-unmapped");
    let parts = run(
        &shard,
        r#"{"size":0,"aggs":{"m":{"multi_terms":{"terms":[{"field":"tag"},{"field":"nope"}]}}}}"#,
    )
    .expect("searches");
    assert!(multi(&parts).buckets.is_empty());
}

// AC-5
#[test]
fn a_shard_minimum_drops_the_buckets_under_it_into_the_other_documents() {
    let shard = shard("multi-min");
    // Documents 0 to 31 by tag alone: each of the sixteen tags twice.
    let parts = run(
        &shard,
        r#"{"size":0,"query":{"range":{"n":{"lt":33}}},"aggs":{"m":{"multi_terms":{
        "terms":[{"field":"tag"},{"field":"tag"}],"min_doc_count":2,"shard_min_doc_count":2}}}}"#,
    )
    .expect("searches");
    let m = multi(&parts);
    assert_eq!(m.buckets.len(), 16);
    // Document 32 has tag t0: three of its documents, the rest two.
    assert_eq!(m.buckets[0], (key_pair("t0"), 3));
    assert_eq!(m.other_doc_count, 0);
    let parts = run(
        &shard,
        r#"{"size":0,"query":{"range":{"n":{"lt":33}}},"aggs":{"m":{"multi_terms":{
        "terms":[{"field":"tag"},{"field":"tag"}],"min_doc_count":3,"shard_min_doc_count":3}}}}"#,
    )
    .expect("searches");
    let m = multi(&parts);
    assert_eq!(m.buckets.len(), 1);
    assert_eq!(m.other_doc_count, 30);
}

fn key_pair(tag: &str) -> MultiKey {
    vec![
        MultiPart::Text(tag.to_owned()),
        MultiPart::Text(tag.to_owned()),
    ]
}

// AC-6
#[test]
fn a_term_the_shard_cannot_read_is_refused() {
    let shard = shard("multi-refused");
    let err = run(
        &shard,
        r#"{"size":0,"aggs":{"m":{"multi_terms":{"terms":[{"field":"tag"},{"field":"x0"}]}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(err.status, 400);
}
