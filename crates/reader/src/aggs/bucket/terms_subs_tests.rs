//! Sub-aggregations of a `terms` over a numeric field, run on a real index
//! of four segments and reduced across shards.
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

use super::sub_values::BucketSubs;
use super::terms::{TermsOrder, TermsResult, TermsSpec, reduce};
use crate::aggs::metric::write_aggregations;
use crate::aggs::partial::{Format, Partial, Value};
use crate::dsl::parse_chunks;
use crate::search::{ShardError, search};
use crate::{OpenShard, open_shard};

const DOCS: usize = 200;

fn shard(name: &str) -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", &Spec::new(name, DOCS).segments_of(50).with_texts(1)) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", name, 0).expect("opens")
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Vec<Partial>, ShardError> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
}

fn terms(shard: &OpenShard<Emulator>, body: &str) -> TermsResult {
    match run(shard, body).expect("searches").remove(0).value {
        Value::Terms(t) => *t,
        other => panic!("not a terms: {other:?}"),
    }
}

// AC-1
#[test]
fn each_numeric_terms_bucket_holds_the_metrics_of_its_documents() {
    let shard = shard("terms-subs");
    // `n` is each document's own number 0 to 199, so every key has one document.
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"n","size":3,"order":{"_key":"asc"}},
            "aggs":{"lo":{"min":{"field":"n"}},"c":{"value_count":{"field":"n"}}}}}}"#,
    );
    assert_eq!(got.buckets, [(0, 1), (1, 1), (2, 1)]);
    let names: Vec<&str> = got.subs.meta.iter().map(|m| m.0.as_str()).collect();
    assert_eq!(names, ["lo", "c"]);
    let want = |k: f64| vec![Value::Min(k), Value::ValueCount(1)];
    assert_eq!(got.subs.lists, [want(0.0), want(1.0), want(2.0)]);
}

// AC-2
#[test]
fn a_keyword_terms_under_a_numeric_terms_counts_that_buckets_documents() {
    let shard = shard("terms-subs-kw");
    let got = terms(
        &shard,
        r#"{"query":{"range":{"n":{"gte":10,"lt":12}}},"aggs":{"t":{"terms":{"field":"n",
            "order":{"_key":"asc"}},"aggs":{"g":{"terms":{"field":"tag"}}}}}}"#,
    );
    let tags: Vec<Vec<(String, u64)>> = got
        .subs
        .lists
        .iter()
        .map(|list| match &list[0] {
            Value::KeywordTerms(t) => t.buckets.clone(),
            other => panic!("not a terms: {other:?}"),
        })
        .collect();
    // `tag` cycles over 16: document 10 holds t10, 11 holds t11.
    assert_eq!(tags, [[("t10".to_owned(), 1)], [("t11".to_owned(), 1)]]);
}

// AC-3
#[test]
fn a_numeric_terms_over_an_unmapped_field_has_no_bucket_and_the_empty_values() {
    let shard = shard("terms-subs-unmapped");
    let got = terms(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"nope"},"aggs":{"c":{"value_count":{"field":"n"}}}}}}"#,
    );
    assert!(got.buckets.is_empty());
    assert!(got.subs.lists.is_empty());
    assert_eq!(got.subs.meta.len(), 1);
}

// AC-4
#[test]
fn a_numeric_terms_under_a_numeric_terms_is_refused() {
    let shard = shard("terms-subs-refused");
    let err = run(
        &shard,
        r#"{"aggs":{"t":{"terms":{"field":"n"},"aggs":{"u":{"terms":{"field":"n"}}}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(err.status, 400);
    assert!(
        format!("{err:?}").contains("sub-aggregation of the numeric field"),
        "{err:?}"
    );
}

// AC-5
fn part(buckets: &[(i64, u64, f64)], spec: &TermsSpec) -> Value {
    Value::Terms(Box::new(TermsResult {
        spec: spec.clone(),
        buckets: buckets.iter().map(|&(k, c, _)| (k, c)).collect(),
        subs: BucketSubs {
            meta: vec![("s".to_owned(), Format::Raw)],
            empty: vec![Value::Sum(0.0)],
            lists: buckets
                .iter()
                .map(|&(_, _, s)| vec![Value::Sum(s)])
                .collect(),
        },
        ..TermsResult::default()
    }))
}

#[test]
fn a_reduce_adds_each_keys_sub_aggregations_and_keeps_them_with_the_kept_buckets() {
    let spec = TermsSpec {
        size: 1,
        shard_size: 10,
        order: TermsOrder::CountDesc,
        ..TermsSpec::default()
    };
    // Key 2 totals 7 documents, key 1 five: only key 2 is kept, with the
    // sum of both shards' values for it.
    let one = part(&[(1, 5, 10.0), (2, 1, 20.0)], &spec);
    let two = part(&[(2, 6, 30.0)], &spec);
    let Some(Value::Terms(r)) = reduce(&[one, two]) else {
        panic!("no reduce");
    };
    assert_eq!(r.buckets, [(2, 7)]);
    assert_eq!(r.subs.lists, [vec![Value::Sum(50.0)]]);
    assert_eq!(r.other_doc_count, 5);
}

// AC-6
#[test]
fn a_buckets_sub_aggregations_follow_its_count_in_the_response() {
    let shard = shard("terms-subs-render");
    let parts = run(
        &shard,
        r#"{"query":{"range":{"n":{"gte":10,"lt":12}}},"aggs":{"t":{"terms":{"field":"n",
            "order":{"_key":"asc"}},"aggs":{"hi":{"max":{"field":"n"}}}}}}"#,
    )
    .expect("searches");
    let mut out = Vec::new();
    write_aggregations(&mut out, &parts).expect("writes");
    assert_eq!(
        String::from_utf8(out).expect("utf-8"),
        concat!(
            r#","aggregations":{"t":{"doc_count_error_upper_bound":0,"sum_other_doc_count":0,"buckets":["#,
            r#"{"key":10,"doc_count":1,"hi":{"value":10.0}},"#,
            r#"{"key":11,"doc_count":1,"hi":{"value":11.0}}]}}"#
        )
    );
}

// AC-7
#[test]
fn a_numeric_terms_reserves_for_the_fields_values_and_a_keyword_one_here_nothing() {
    use crate::aggs::bucket::keyed_subs;
    let shard = shard("terms-subs-reserved");
    let aggs = |body: &str| parse_chunks([body.as_bytes()]).expect("parses").aggs;
    let numeric =
        aggs(r#"{"aggs":{"t":{"terms":{"field":"n"},"aggs":{"m":{"min":{"field":"n"}}}}}}"#);
    let per_bucket = std::mem::size_of::<lucene_aggs::metric_aggs::MetricState>()
        + 3 * std::mem::size_of::<Value>();
    assert_eq!(keyed_subs::reserved(&shard, &numeric), DOCS * per_bucket);
    // A keyword `terms` reserves in `keyword_subs`.
    let keyword =
        aggs(r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"u":{"terms":{"field":"tag"}}}}}}"#);
    assert_eq!(keyed_subs::reserved(&shard, &keyword), 0);
}
