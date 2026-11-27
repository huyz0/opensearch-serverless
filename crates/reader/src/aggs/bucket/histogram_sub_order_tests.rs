//! `histogram` and `date_histogram` ordered by a sub-aggregation.
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
use super::sub_order::SubOrder;
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

/// The answer's `buckets` as written, after the reduce.
fn written(parts: Vec<Partial>) -> String {
    let parts: Vec<Partial> = parts
        .into_iter()
        .map(|p| Partial {
            value: crate::aggs::partial::reduce(&[p.value]).expect("reduces"),
            ..p
        })
        .collect();
    let mut out = Vec::new();
    write_aggregations(&mut out, &parts).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

/// The keys of the written buckets, in order.
fn keys(json: &str) -> Vec<i64> {
    json.split("\"key\":")
        .skip(1)
        .map(|rest| {
            let end = rest
                .find(|c: char| !(c.is_ascii_digit() || c == '-'))
                .unwrap_or(rest.len());
            rest[..end].parse().expect("a key")
        })
        .collect()
}

fn reason(body: &str) -> String {
    let body = format!(r#"{{"aggs":{{"h":{{"histogram":{body}}}}}}}"#);
    parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure()
        .reason
        .clone()
}

// AC-1
#[test]
fn an_order_by_a_path_parses_to_the_path_and_direction() {
    let body = br#"{"aggs":{"h":{"histogram":{"field":"n","interval":50,"order":{"s.max":"asc"}},
        "aggs":{"s":{"stats":{"field":"n"}}}}}}"#;
    let agg = parse_chunks([&body[..]]).expect("parses").aggs.remove(0);
    let spec = agg.histogram.expect("a histogram");
    assert_eq!(
        spec.sub_order,
        Some(SubOrder {
            path: "s.max".to_owned(),
            ascending: true
        })
    );
    let body = br#"{"aggs":{"h":{"date_histogram":{"field":"n","fixed_interval":"50ms",
        "order":[{"m":"desc"}]},"aggs":{"m":{"max":{"field":"n"}}}}}}"#;
    let agg = parse_chunks([&body[..]]).expect("parses").aggs.remove(0);
    let spec = agg.date_histogram.expect("a date_histogram");
    assert_eq!(spec.sub_order.map(|o| o.ascending), Some(false));
    // The keyed orders keep no sub-order.
    let body =
        br#"{"aggs":{"h":{"histogram":{"field":"n","interval":50,"order":{"_count":"asc"}}}}}"#;
    let agg = parse_chunks([&body[..]]).expect("parses").aggs.remove(0);
    assert_eq!(agg.histogram.expect("a histogram").sub_order, None);
}

// AC-1
#[test]
fn a_bad_path_fails_as_it_does_for_a_terms() {
    let got = reason(r#"{"field":"n","interval":1,"order":{"[":"asc"}}"#);
    assert!(got.contains("Invalid path element"), "{got}");
    let got = reason(r#"{"field":"n","interval":1,"order":{">":"asc"}}"#);
    assert!(got.contains("Invalid path []"), "{got}");
}

// AC-2
#[test]
fn buckets_list_by_the_targets_value_in_either_direction() {
    let shard = shard("hist-sub-order");
    let by = |order: &str| {
        keys(&written(
            run(
                &shard,
                &format!(
                    r#"{{"size":0,"aggs":{{"h":{{"histogram":{{"field":"n","interval":50,"order":{order}}},
                    "aggs":{{"hi":{{"max":{{"field":"n"}}}},"s":{{"stats":{{"field":"n"}}}},
                             "c":{{"value_count":{{"field":"n"}}}}}}}}}}}}"#
                ),
            )
            .expect("searches"),
        ))
    };
    assert_eq!(by(r#"{"hi":"desc"}"#), [150, 100, 50, 0]);
    assert_eq!(by(r#"{"hi":"asc"}"#), [0, 50, 100, 150]);
    assert_eq!(by(r#"{"s.min":"desc"}"#), [150, 100, 50, 0]);
    // Every bucket counts 50: the tie lists by key ascending, either way.
    assert_eq!(by(r#"{"c":"desc"}"#), [0, 50, 100, 150]);
    assert_eq!(by(r#"{"c":"asc"}"#), [0, 50, 100, 150]);
}

// AC-5
#[test]
fn a_date_histogram_is_ordered_the_same_way() {
    let shard = shard("date-sub-order");
    let got = keys(&written(
        run(
            &shard,
            r#"{"size":0,"aggs":{"h":{"date_histogram":{"field":"n","fixed_interval":"50ms",
                "order":{"hi":"desc"}},"aggs":{"hi":{"max":{"field":"n"}}}}}}"#,
        )
        .expect("searches"),
    ));
    assert_eq!(got, [150, 100, 50, 0]);
}

// AC-4
#[test]
fn a_target_the_shard_cannot_order_by_fails_it() {
    let shard = shard("hist-sub-order-bad");
    for (order, want) in [
        (
            r#"{"nope":"asc"}"#,
            "either does not exist, or is a pipeline aggregation",
        ),
        (
            r#"{"s":"asc"}"#,
            "When ordering on a multi-value metrics aggregation a metric name must be specified",
        ),
    ] {
        let e = run(
            &shard,
            &format!(
                r#"{{"aggs":{{"h":{{"histogram":{{"field":"n","interval":50,"order":{order}}},
                "aggs":{{"s":{{"stats":{{"field":"n"}}}}}}}}}}}}"#
            ),
        )
        .expect_err("fails");
        assert_eq!(e.status, 500);
        assert!(e.failure.reason.contains(want), "{}", e.failure.reason);
    }
}

// AC-3
#[test]
fn the_reduce_orders_by_the_reduced_values_and_gap_buckets_sort_last() {
    let spec = HistogramSpec {
        interval: 10.0,
        sub_order: Some(SubOrder {
            path: "m".to_owned(),
            ascending: false,
        }),
        ..HistogramSpec::default()
    };
    let part = |buckets: &[(f64, u64)], maxes: &[f64]| HistogramResult {
        spec: spec.clone(),
        buckets: buckets.to_vec(),
        subs: BucketSubs {
            meta: vec![("m".to_owned(), Format::Raw)],
            empty: vec![Value::Max(f64::NEG_INFINITY)],
            lists: maxes.iter().map(|m| vec![Value::Max(*m)]).collect(),
        },
    };
    // Key 10 holds 5 on one shard and 9 on the other: 9 wins; key 20 is a gap.
    let a = part(&[(0.0, 1), (10.0, 1), (30.0, 1)], &[3.0, 5.0, 1.0]);
    let b = part(&[(10.0, 1)], &[9.0]);
    let Some(Value::Histogram(got)) =
        reduce(&[Value::Histogram(Box::new(a)), Value::Histogram(Box::new(b))])
    else {
        panic!("not a histogram");
    };
    let at: Vec<f64> = got.buckets.iter().map(|b| b.0).collect();
    assert_eq!(at, [10.0, 0.0, 30.0, 20.0]);
    assert_eq!(got.subs.lists.len(), 4);
    assert_eq!(got.subs.lists[0], [Value::Max(9.0)]);
}
