//! `date_range` parsed and run.
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
use crate::aggs::partial::Value;
use crate::dsl::parse_chunks;
use crate::search::search;
use crate::{OpenShard, open_shard};

fn parsed(body: &str) -> Result<crate::aggs::metric::MetricAgg, String> {
    let body = format!(r#"{{"aggs":{{"d":{{"date_range":{body}}}}}}}"#);
    parse_chunks([body.as_bytes()])
        .map(|mut s| s.aggs.remove(0))
        .map_err(|e| {
            let failure = e.into_failure();
            let mut text = failure.reason.clone();
            if let Some(cause) = &failure.cause {
                text.push_str(" | ");
                text.push_str(&cause.reason);
            }
            text
        })
}

// AC-1
#[test]
fn text_bounds_are_dates_and_numbers_are_milliseconds() {
    let agg = parsed(
        r#"{"field":"n","ranges":[{"to":"1970-01-01T00:00:00.050Z"},
            {"from":"1970-01-01T00:00:00.050Z","to":100},{"from":"1970-01-02"}]}"#,
    )
    .expect("parses");
    assert_eq!(agg.metric, Metric::DateRange);
    assert_eq!(Metric::from_name("date_range"), Some(Metric::DateRange));
    assert_eq!(Metric::DateRange.name(), "date_range");
    let r = agg.range.expect("a range");
    let bounds: Vec<(f64, f64)> = r.ranges.iter().map(|d| (d.from, d.to)).collect();
    assert_eq!(
        bounds,
        [
            (f64::NEG_INFINITY, 50.0),
            (50.0, 100.0),
            (86_400_000.0, f64::INFINITY)
        ]
    );
}

// AC-1
#[test]
fn bad_bodies_and_unserved_options_fail_by_name() {
    let got = parsed(r#"{"field":"n","ranges":[{"from":"not a date"}]}"#).expect_err("refused");
    assert!(
        got.contains("[date_range] failed to parse field [ranges]"),
        "{got}"
    );
    let got = parsed(r#"{"field":"n","nope":1,"ranges":[{"to":1}]}"#).expect_err("refused");
    assert!(got.contains("[date_range] unknown field [nope]"), "{got}");
    let got = parsed(r#"{"field":"n"}"#).expect_err("refused");
    assert!(
        got.contains("No [ranges] specified for the [d] aggregation"),
        "{got}"
    );
    for body in [
        r#"{"field":"n","format":"yyyy","ranges":[{"to":1}]}"#,
        r#"{"field":"n","time_zone":"+01:00","ranges":[{"to":1}]}"#,
        r#"{"field":"n","ranges":[{"to":"now-1d"}]}"#,
        r#"{"field":"n","ranges":[{"to":"2020-01-01||+1d"}]}"#,
    ] {
        let got = parsed(body).expect_err("refused");
        assert!(
            got.contains("not supported by this node yet"),
            "{body}: {got}"
        );
    }
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

// AC-2, AC-3
#[test]
fn each_range_counts_the_documents_between_its_dates() {
    let shard = shard("date-range");
    let source = parse_chunks([
        br#"{"size":0,"aggs":{"d":{"date_range":{"field":"n","ranges":[
        {"to":"1970-01-01T00:00:00.010Z"},
        {"from":"1970-01-01T00:00:00.010Z","to":60},
        {"from":150}]}}}}"#
            .as_slice(),
    ])
    .expect("parses");
    let mut parts = search(&shard, "idx", &source, &Analyzer::standard(None))
        .expect("searches")
        .aggregations;
    let Value::Range(r) = parts.remove(0).value else {
        panic!("not a range");
    };
    // `n` is each document's own number 0 to 199.
    let counts: Vec<u64> = r.buckets.iter().map(|b| b.doc_count).collect();
    assert_eq!(counts, [10, 50, 50]);
}
