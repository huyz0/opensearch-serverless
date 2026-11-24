//! Sub-aggregations of `histogram` and `date_histogram`, run on a real index
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

use super::histogram::{HistogramResult, HistogramSpec, reduce};
use super::sub_values::BucketSubs;
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

fn first(shard: &OpenShard<Emulator>, body: &str) -> Value {
    run(shard, body).expect("searches").remove(0).value
}

fn written(parts: &[Partial]) -> String {
    let mut out = Vec::new();
    write_aggregations(&mut out, parts).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

// AC-1
#[test]
fn each_histogram_bucket_holds_the_metrics_of_its_documents() {
    let shard = shard("keyed-hist");
    // `n` is each document's own number 0 to 199.
    let Value::Histogram(h) = first(
        &shard,
        r#"{"size":0,"aggs":{"h":{"histogram":{"field":"n","interval":50},
            "aggs":{"lo":{"min":{"field":"n"}},"hi":{"max":{"field":"n"}},
                    "c":{"value_count":{"field":"n"}}}}}}"#,
    ) else {
        panic!("not a histogram");
    };
    assert_eq!(h.buckets, [(0.0, 50), (50.0, 50), (100.0, 50), (150.0, 50)]);
    let names: Vec<&str> = h.subs.meta.iter().map(|m| m.0.as_str()).collect();
    assert_eq!(names, ["lo", "hi", "c"]);
    let want = |lo: f64, hi: f64| vec![Value::Min(lo), Value::Max(hi), Value::ValueCount(50)];
    assert_eq!(
        h.subs.lists,
        [
            want(0.0, 49.0),
            want(50.0, 99.0),
            want(100.0, 149.0),
            want(150.0, 199.0)
        ]
    );
    assert_eq!(
        h.subs.empty,
        [
            Value::Min(f64::INFINITY),
            Value::Max(f64::NEG_INFINITY),
            Value::ValueCount(0)
        ]
    );
}

#[test]
fn each_date_histogram_bucket_holds_the_metrics_of_its_documents() {
    let shard = shard("keyed-date");
    let Value::DateHistogram(h) = first(
        &shard,
        r#"{"aggs":{"h":{"date_histogram":{"field":"n","fixed_interval":"100ms"},
            "aggs":{"s":{"sum":{"field":"n"}}}}}}"#,
    ) else {
        panic!("not a date_histogram");
    };
    assert_eq!(h.buckets, [(0, 100), (100, 100)]);
    // 0 + ... + 99 and 100 + ... + 199.
    assert_eq!(
        h.subs.lists,
        [vec![Value::Sum(4950.0)], vec![Value::Sum(14950.0)]]
    );
}

// AC-2
#[test]
fn a_keyword_terms_under_a_histogram_counts_that_buckets_documents() {
    let shard = shard("keyed-terms");
    let Value::Histogram(h) = first(
        &shard,
        r#"{"aggs":{"h":{"histogram":{"field":"n","interval":100},
            "aggs":{"t":{"terms":{"field":"tag","size":1}}}}}}"#,
    ) else {
        panic!("not a histogram");
    };
    let top = |bucket: usize| match &h.subs.lists[bucket][0] {
        Value::KeywordTerms(t) => t.buckets[0].clone(),
        other => panic!("not a terms: {other:?}"),
    };
    // `tag` cycles over 16: documents 0..99 hold t0 to t3 seven times, t4 to
    // t15 six; the first term by count then key is t0 in both halves.
    assert_eq!(top(0), ("t0".to_owned(), 7));
    assert_eq!(top(1), ("t0".to_owned(), 6));
}

// AC-3
#[test]
fn a_histogram_over_an_unmapped_field_has_no_bucket_and_the_empty_values() {
    let shard = shard("keyed-unmapped");
    let Value::Histogram(h) = first(
        &shard,
        r#"{"aggs":{"h":{"histogram":{"field":"nope","interval":10},
            "aggs":{"c":{"value_count":{"field":"n"}}}}}}"#,
    ) else {
        panic!("not a histogram");
    };
    assert!(h.buckets.is_empty());
    assert!(h.subs.lists.is_empty());
    assert_eq!(h.subs.meta.len(), 1);
    assert_eq!(h.subs.empty, [Value::ValueCount(0)]);
}

// AC-4
#[test]
fn a_bucket_sub_aggregation_under_a_histogram_is_refused() {
    let body = r#"{"aggs":{"h":{"histogram":{"field":"n","interval":5},
        "aggs":{"r":{"percentiles":{"field":"n"}}}}}}"#;
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        failure
            .reason
            .contains("[percentiles] sub-aggregation of a [histogram]"),
        "{}",
        failure.reason
    );
}

// AC-5
fn part(interval: f64, buckets: &[(f64, u64, f64)], min_doc_count: u64) -> HistogramResult {
    HistogramResult {
        spec: HistogramSpec {
            interval,
            min_doc_count,
            ..HistogramSpec::default()
        },
        buckets: buckets.iter().map(|&(k, c, _)| (k, c)).collect(),
        subs: BucketSubs {
            meta: vec![("s".to_owned(), Format::Raw)],
            empty: vec![Value::Sum(0.0)],
            lists: buckets
                .iter()
                .map(|&(_, _, s)| vec![Value::Sum(s)])
                .collect(),
        },
    }
}

#[test]
fn a_reduce_adds_each_keys_sub_aggregations_and_fills_the_gaps_with_the_empty_value() {
    let one = Value::Histogram(Box::new(part(10.0, &[(0.0, 1, 5.0), (30.0, 2, 7.0)], 0)));
    let two = Value::Histogram(Box::new(part(10.0, &[(30.0, 3, 11.0)], 0)));
    let Some(Value::Histogram(r)) = reduce(&[one, two]) else {
        panic!("no reduce");
    };
    // Keys 10 and 20 are gaps a reduce fills.
    assert_eq!(r.buckets, [(0.0, 1), (10.0, 0), (20.0, 0), (30.0, 5)]);
    assert_eq!(
        r.subs.lists,
        [
            vec![Value::Sum(5.0)],
            vec![Value::Sum(0.0)],
            vec![Value::Sum(0.0)],
            vec![Value::Sum(18.0)]
        ]
    );
}

#[test]
fn a_reduce_keeps_each_buckets_values_with_its_bucket_in_the_ordered_answer() {
    let mut spec_part = part(10.0, &[(0.0, 1, 5.0), (10.0, 9, 7.0)], 1);
    spec_part.spec.order = super::histogram::HistogramOrder::CountDesc;
    let Some(Value::Histogram(r)) = reduce(&[Value::Histogram(Box::new(spec_part))]) else {
        panic!("no reduce");
    };
    assert_eq!(r.buckets, [(10.0, 9), (0.0, 1)]);
    assert_eq!(r.subs.lists, [vec![Value::Sum(7.0)], vec![Value::Sum(5.0)]]);
}

// AC-6
#[test]
fn a_buckets_sub_aggregations_follow_its_count_in_the_response() {
    let shard = shard("keyed-render");
    let parts = run(
        &shard,
        r#"{"query":{"range":{"n":{"lt":100}}},"aggs":{"h":{"histogram":{"field":"n","interval":50},
            "aggs":{"hi":{"max":{"field":"n"}}}}}}"#,
    )
    .expect("searches");
    assert_eq!(
        written(&parts),
        concat!(
            r#","aggregations":{"h":{"buckets":["#,
            r#"{"key":0.0,"doc_count":50,"hi":{"value":49.0}},"#,
            r#"{"key":50.0,"doc_count":50,"hi":{"value":99.0}}]}}"#
        )
    );
}

// AC-7
#[test]
fn a_histograms_sub_aggregations_reserve_for_the_fields_values() {
    use crate::aggs::bucket::keyed_subs;
    let shard = shard("keyed-reserved");
    let aggs = |body: &str| parse_chunks([body.as_bytes()]).expect("parses").aggs;
    let with = aggs(
        r#"{"aggs":{"h":{"histogram":{"field":"n","interval":5},
            "aggs":{"m":{"min":{"field":"n"}}}}}}"#,
    );
    let per_bucket = std::mem::size_of::<lucene_aggs::metric_aggs::MetricState>()
        + 3 * std::mem::size_of::<Value>();
    // `n` holds one value in each of the 200 documents.
    assert_eq!(keyed_subs::reserved(&shard, &with), DOCS * per_bucket);
    let plain = aggs(r#"{"aggs":{"h":{"histogram":{"field":"n","interval":5}}}}"#);
    assert_eq!(keyed_subs::reserved(&shard, &plain), 0);
    let unmapped = aggs(
        r#"{"aggs":{"h":{"histogram":{"field":"nope","interval":5},
            "aggs":{"m":{"min":{"field":"n"}}}}}}"#,
    );
    assert_eq!(keyed_subs::reserved(&shard, &unmapped), 0);
    let fixed = aggs(
        r#"{"aggs":{"r":{"range":{"field":"n","ranges":[{"to":5}]},
            "aggs":{"m":{"min":{"field":"n"}}}}}}"#,
    );
    assert_eq!(keyed_subs::reserved(&shard, &fixed), 0);
}

#[test]
fn the_search_reserves_a_histograms_sub_aggregations_with_its_hits() {
    let shard = shard("keyed-hits");
    let bytes = |body: &str| {
        let source = parse_chunks([body.as_bytes()]).expect("parses");
        crate::search::hits_bytes(&source, &shard)
    };
    let plain = bytes(r#"{"aggs":{"h":{"histogram":{"field":"n","interval":5}}}}"#);
    let with = bytes(
        r#"{"aggs":{"h":{"histogram":{"field":"n","interval":5},
            "aggs":{"m":{"min":{"field":"n"}}}}}}"#,
    );
    assert!(with > plain);
}
