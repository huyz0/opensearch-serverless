//! `rare_terms` parsed, reduced, written and run.
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

use crate::aggs::bucket::rare::{RareKey, RareResult, RareSpec, reduce, write_rare};
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

fn spec_of(body: &str) -> RareSpec {
    let agg = parse_chunks([body.as_bytes()])
        .expect("parses")
        .aggs
        .remove(0);
    assert_eq!(agg.metric, Metric::RareTerms);
    *agg.rare.expect("a spec")
}

// AC-1
#[test]
fn the_body_parses_with_its_defaults_and_options() {
    let spec = spec_of(r#"{"aggs":{"r":{"rare_terms":{"field":"tag"}}}}"#);
    assert_eq!((spec.max_doc_count, spec.precision), (1, 0.001));
    let spec = spec_of(
        r#"{"aggs":{"r":{"rare_terms":{"field":"tag","max_doc_count":"5","precision":0.01}}}}"#,
    );
    assert_eq!((spec.max_doc_count, spec.precision), (5, 0.01));
}

// AC-1
#[test]
fn bad_and_unserved_options_fail_as_java_fails() {
    for (body, want) in [
        (
            r#"{"field":"tag","max_doc_count":0}"#,
            "[max_doc_count] must be greater than 0. Found [0] in [r]",
        ),
        (
            r#"{"field":"tag","max_doc_count":101}"#,
            "[max_doc_count] must be smallerthan 100in [r]",
        ),
        (
            r#"{"field":"tag","precision":0.000001}"#,
            "[precision] must be greater than 0.00001",
        ),
        (
            r#"{"max_doc_count":2}"#,
            "Required one of fields [field, script]",
        ),
        (
            r#"{"field":"tag","nope":1}"#,
            "[rare_terms] unknown field [nope]",
        ),
        (
            r#"{"field":"tag","include":"a.*"}"#,
            "not supported by this node yet",
        ),
        (
            r#"{"field":"tag","missing":"x"}"#,
            "not supported by this node yet",
        ),
        (
            r#"{"field":"tag","max_doc_count":[1]}"#,
            "doesn't support values of type",
        ),
    ] {
        let got = reason(&format!(r#"{{"aggs":{{"r":{{"rare_terms":{body}}}}}}}"#));
        assert!(got.contains(want), "{body}: {got}");
    }
}

// AC-5
#[test]
fn sub_aggregations_and_nesting_are_refused() {
    let got = reason(
        r#"{"aggs":{"r":{"rare_terms":{"field":"tag"},"aggs":{"m":{"max":{"field":"n"}}}}}}"#,
    );
    assert!(got.contains("sub-aggregation"), "{got}");
    let got = reason(
        r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"r":{"rare_terms":{"field":"tag"}}}}}}"#,
    );
    assert!(got.contains("sub-aggregation"), "{got}");
}

fn text(t: &str) -> RareKey {
    RareKey::Text(t.to_owned())
}

fn result(max: u64, buckets: &[(&str, u64)], excluded: &[&str]) -> Value {
    Value::Rare(Box::new(RareResult {
        spec: RareSpec {
            max_doc_count: max,
            ..RareSpec::default()
        },
        long: false,
        buckets: buckets.iter().map(|(t, n)| (text(t), *n)).collect(),
        excluded: excluded.iter().map(|t| text(t)).collect(),
    }))
}

fn reduced(parts: &[Value]) -> Vec<(String, u64)> {
    let Some(Value::Rare(r)) = reduce(parts) else {
        panic!("not reduced");
    };
    r.buckets
        .into_iter()
        .map(|(k, n)| match k {
            RareKey::Text(t) => (t, n),
            RareKey::Long(v) => (v.to_string(), n),
        })
        .collect()
}

// AC-3
#[test]
fn a_reduce_adds_counts_and_drops_what_any_shard_excluded() {
    let a = result(2, &[("a", 1), ("b", 1), ("c", 2)], &["x"]);
    let b = result(2, &[("a", 1), ("x", 1), ("d", 1)], &["b"]);
    // `a` is 2 in all; `b` was common on the second shard; `x` on the first;
    // `c` is 2; `d` is 1.
    assert_eq!(
        reduced(&[a, b]),
        [
            ("d".to_owned(), 1),
            ("a".to_owned(), 2),
            ("c".to_owned(), 2)
        ]
    );
}

// AC-3
#[test]
fn a_reduce_drops_a_term_whose_sum_passes_the_maximum() {
    let a = result(1, &[("a", 1), ("b", 1)], &[]);
    let b = result(1, &[("a", 1)], &[]);
    assert_eq!(reduced(&[a, b]), [("b".to_owned(), 1)]);
    assert_eq!(reduce(&[]), None);
    assert_eq!(reduce(&[Value::Min(1.0)]), None);
}

// AC-4
#[test]
fn the_response_lists_each_bucket_by_key_and_count() {
    let Value::Rare(r) = result(2, &[("b", 2), ("a\"", 1)], &[]) else {
        unreachable!()
    };
    let mut out = Vec::new();
    write_rare(&mut out, &r).expect("writes");
    assert_eq!(
        String::from_utf8(out).expect("utf-8"),
        r#""buckets":[{"key":"a\"","doc_count":1},{"key":"b","doc_count":2}]"#
    );
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

// AC-2, AC-4
#[test]
fn a_shard_keeps_the_terms_of_few_documents_and_excludes_the_others() {
    let shard = shard("rare-run");
    // Documents 0 to 19: tags t0 to t3 twice, the twelve others once.
    let body = r#"{"size":0,"query":{"range":{"n":{"lt":20}}},"aggs":{
        "r":{"rare_terms":{"field":"tag"}}}}"#;
    let parts = run(&shard, body).expect("searches");
    let Value::Rare(r) = &parts[0].value else {
        panic!("not rare");
    };
    assert_eq!(r.buckets.len(), 12);
    assert!(r.buckets.iter().all(|(_, n)| *n == 1));
    assert_eq!(r.excluded, [text("t0"), text("t1"), text("t2"), text("t3")]);
    let wider = body.replace(r#""field":"tag""#, r#""field":"tag","max_doc_count":2"#);
    let parts = run(&shard, &wider).expect("searches");
    let got = reduced(&[parts[0].value.clone()]);
    assert_eq!(got.len(), 16);
    assert_eq!(got[0].1, 1);
    assert_eq!(got[15], ("t3".to_owned(), 2));
}

// AC-4
#[test]
fn an_unmapped_field_answers_nothing() {
    let shard = shard("rare-unmapped");
    let parts = run(
        &shard,
        r#"{"size":0,"aggs":{"r":{"rare_terms":{"field":"nope"}}}}"#,
    )
    .expect("searches");
    let Value::Rare(r) = &parts[0].value else {
        panic!("not rare");
    };
    assert!(r.buckets.is_empty() && r.excluded.is_empty());
}

// AC-6
#[test]
fn a_long_field_is_answered_with_numbers_as_lrareterms() {
    let shard = shard("rare-long");
    let parts = run(
        &shard,
        r#"{"size":0,"query":{"range":{"n":{"lt":3}}},"aggs":{"r":{"rare_terms":{"field":"n"}}}}"#,
    )
    .expect("searches");
    let Value::Rare(r) = &parts[0].value else {
        panic!("not rare");
    };
    assert!(r.long && r.excluded.is_empty());
    assert_eq!(r.type_name(), "lrareterms");
    let mut out = Vec::new();
    write_rare(&mut out, r).expect("writes");
    assert_eq!(
        String::from_utf8(out).expect("utf-8"),
        r#""buckets":[{"key":0,"doc_count":1},{"key":1,"doc_count":1},{"key":2,"doc_count":1}]"#
    );
    // A keyword field is `srareterms`, even with no bucket.
    let parts = run(
        &shard,
        r#"{"size":0,"aggs":{"r":{"rare_terms":{"field":"nope"}}}}"#,
    )
    .expect("searches");
    let Value::Rare(r) = &parts[0].value else {
        panic!("not rare");
    };
    assert_eq!(r.type_name(), "srareterms");
}
