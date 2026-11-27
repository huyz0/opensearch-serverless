//! `global` parsed and run beside aggregations over the query.
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

use crate::aggs::metric::Metric;
use crate::aggs::partial::{Partial, Value};
use crate::dsl::{Query, parse_chunks};
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

// AC-1
#[test]
fn a_global_body_is_empty_and_means_match_all() {
    let body = br#"{"aggs":{"g":{"global":{}}}}"#;
    let agg = parse_chunks([&body[..]]).expect("parses").aggs.remove(0);
    assert_eq!(agg.metric, Metric::Global);
    assert_eq!(Metric::from_name("global"), Some(Metric::Global));
    assert_eq!(Metric::Global.name(), "global");
    let spec = agg.filters.expect("a filter");
    assert!(spec.single);
    assert!(matches!(spec.filters[0].1, Query::MatchAll { .. }));
    let got = reason(r#"{"aggs":{"g":{"global":{"x":1}}}}"#);
    assert!(got.contains("Unexpected token FIELD_NAME in [g]."), "{got}");
}

// AC-3
#[test]
fn a_global_is_top_level_only_and_holds_no_top_hits() {
    let got = reason(r#"{"aggs":{"f":{"filter":{"match_all":{}},"aggs":{"g":{"global":{}}}}}}"#);
    assert!(
        got.contains("[global] sub-aggregation of a [filter]"),
        "{got}"
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

// AC-2
#[test]
fn a_global_counts_every_document_whatever_the_query_and_keeps_request_order() {
    let shard = shard("global-run");
    let parts = run(
        &shard,
        r#"{"size":0,"query":{"range":{"n":{"lt":10}}},"aggs":{
            "before":{"value_count":{"field":"n"}},
            "all":{"global":{},"aggs":{"low":{"filter":{"range":{"n":{"lt":20}}}}}},
            "after":{"max":{"field":"n"}}}}"#,
    )
    .expect("searches");
    let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["before", "all", "after"]);
    assert_eq!(parts[0].value, Value::ValueCount(10));
    assert_eq!(parts[2].value, Value::Max(9.0));
    let Value::Filters(all) = &parts[1].value else {
        panic!("not a filter");
    };
    assert_eq!(all.buckets[0].1, 200);
    let Value::Filters(low) = &all.subs[0][0] else {
        panic!("not a filter");
    };
    assert_eq!(low.buckets[0].1, 20);
}

// AC-3
#[test]
fn a_top_hits_under_a_global_is_refused() {
    let shard = shard("global-hits");
    let e = run(
        &shard,
        r#"{"aggs":{"g":{"global":{},"aggs":{"h":{"top_hits":{"size":1}}}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(e.status, 400);
    assert!(e.failure.reason.contains("not supported by this node yet"));
}
