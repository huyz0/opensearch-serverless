//! Metric aggregations run on a real index of four segments.
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

use crate::aggs::metric::{Partial, Value};
use crate::dsl::parse_chunks;
use crate::search::{ShardError, search};
use crate::{OpenShard, open_shard};

fn source(body: &str) -> crate::dsl::SearchSource {
    parse_chunks([body.as_bytes()]).expect("parses")
}

const DOCS: usize = 200;

fn shard(name: &str) -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    // Four segments, so each state carries across segments.
    for (key, bytes) in blobs("", &Spec::new(name, DOCS).segments_of(50).with_texts(1)) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", name, 0).expect("opens")
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Vec<Partial>, ShardError> {
    let source = source(body);
    search(shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
}

fn values(shard: &OpenShard<Emulator>, body: &str) -> Vec<Value> {
    run(shard, body)
        .expect("searches")
        .into_iter()
        .map(|p| p.value)
        .collect()
}

#[test]
fn metrics_of_every_match_across_segments() {
    let shard = shard("aggs-all");
    // Document i has n = i, so the values are 0 to 199.
    let got = values(
        &shard,
        r#"{"size":0,"aggs":{"a":{"min":{"field":"n"}},"b":{"max":{"field":"n"}},
            "c":{"sum":{"field":"n"}},"d":{"avg":{"field":"n"}},
            "e":{"stats":{"field":"n"}},"f":{"value_count":{"field":"n"}}}}"#,
    );
    assert_eq!(
        got,
        [
            Value::Min(0.0),
            Value::Max(199.0),
            Value::Sum(19_900.0),
            Value::Avg {
                sum: 19_900.0,
                count: 200
            },
            Value::Stats {
                count: 200,
                sum: 19_900.0,
                min: 0.0,
                max: 199.0
            },
            Value::ValueCount(200),
        ]
    );
}

#[test]
fn metrics_of_the_query_matches_only() {
    let shard = shard("aggs-range");
    let got = values(
        &shard,
        r#"{"query":{"range":{"n":{"gte":10,"lt":20}}},
            "aggs":{"s":{"stats":{"field":"n"}},"t":{"min":{"field":"n"}}}}"#,
    );
    assert_eq!(
        got,
        [
            Value::Stats {
                count: 10,
                sum: 145.0,
                min: 10.0,
                max: 19.0
            },
            Value::Min(10.0),
        ]
    );
    // A query on the keyword: every sixteenth document.
    let got = values(
        &shard,
        r#"{"query":{"term":{"tag":"t3"}},"aggs":{"c":{"value_count":{"field":"n"}},
            "s":{"sum":{"field":"n"}}}}"#,
    );
    let tagged: Vec<usize> = (0..DOCS).filter(|i| i % 16 == 3).collect();
    let sum: usize = tagged.iter().sum();
    assert_eq!(
        got,
        [
            Value::ValueCount(tagged.len() as u64),
            Value::Sum(sum as f64)
        ]
    );
}

#[test]
fn an_unmapped_field_or_no_match_aggregates_nothing() {
    let shard = shard("aggs-none");
    let empty = [
        Value::Min(f64::INFINITY),
        Value::Max(f64::NEG_INFINITY),
        Value::Sum(0.0),
        Value::Avg { sum: 0.0, count: 0 },
        Value::Stats {
            count: 0,
            sum: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        },
        Value::ValueCount(0),
    ];
    for field in ["missing_field", "n"] {
        let query = if field == "n" {
            r#""query":{"term":{"tag":"none"}},"#
        } else {
            ""
        };
        let body = format!(
            r#"{{{query}"aggs":{{"a":{{"min":{{"field":"{field}"}}}},"b":{{"max":{{"field":"{field}"}}}},
            "c":{{"sum":{{"field":"{field}"}}}},"d":{{"avg":{{"field":"{field}"}}}},
            "e":{{"stats":{{"field":"{field}"}}}},"f":{{"value_count":{{"field":"{field}"}}}}}}}}"#
        );
        assert_eq!(values(&shard, &body), empty, "{field}");
    }
}

#[test]
fn fields_a_metric_cannot_read_fail_the_shard() {
    let shard = shard("aggs-refused");
    for (body, want) in [
        (
            r#"{"aggs":{"a":{"min":{"field":"tag"}}}}"#,
            "Field [tag] of type [keyword] is not supported for aggregation [min]",
        ),
        (
            r#"{"aggs":{"a":{"stats":{"field":"x0"}}}}"#,
            "Text fields are not optimised for operations that require per-document field data \
             like aggregations and sorting, so these operations are disabled by default. Please \
             use a keyword field instead. Alternatively, set fielddata=true on [x0] in order to \
             load field data by uninverting the inverted index. Note that this can use \
             significant memory.",
        ),
        (
            r#"{"aggs":{"a":{"value_count":{"field":"tag"}}}}"#,
            "[value_count] of the keyword field [tag] is not supported by this node yet",
        ),
    ] {
        let e = run(&shard, body).expect_err("refused");
        assert_eq!(e.status, 400, "{body}");
        assert_eq!(e.failure.reason, want, "{body}");
    }
}

#[test]
fn a_search_reserves_its_aggregation_state_with_its_hits() {
    let shard = shard("aggs-bytes");
    let plain = source(r#"{"size":0}"#);
    let with_aggs =
        source(r#"{"size":0,"aggs":{"a":{"min":{"field":"n"}},"b":{"stats":{"field":"n"}}}}"#);
    assert_eq!(crate::search::hits_bytes(&plain, &shard), 0);
    assert_eq!(
        crate::search::hits_bytes(&with_aggs, &shard),
        crate::aggs::metric::state_bytes(&with_aggs.aggs)
    );
    assert!(crate::aggs::metric::state_bytes(&with_aggs.aggs) > 0);
}

#[test]
fn fields_the_node_cannot_read_yet_or_without_doc_values_fail_the_shard() {
    let mut shard = shard("aggs-types");
    shard.meta.mapping = crate::mapping::Mapping::from_json(
        r#"{"properties":{"n":{"type":"long","doc_values":false},"d":{"type":"double"},
            "b":{"type":"byte"}}}"#,
    )
    .expect("mapping");
    let refused = |body: &str| run(&shard, body).expect_err("refused");
    let e = refused(r#"{"aggs":{"a":{"sum":{"field":"n"}}}}"#);
    assert_eq!(e.status, 400);
    assert_eq!(
        e.failure.reason,
        "Can't load fielddata on [n] because fielddata is unsupported on fields of type [long]. \
         Use doc values instead."
    );
    let e = refused(r#"{"aggs":{"a":{"avg":{"field":"d"}}}}"#);
    assert_eq!(e.status, 400);
    assert!(
        e.failure
            .reason
            .ends_with("[d] is not supported by this node yet"),
        "{}",
        e.failure.reason
    );
    // A `byte` field the index does not hold aggregates nothing.
    assert_eq!(
        values(&shard, r#"{"aggs":{"a":{"max":{"field":"b"}}}}"#),
        [Value::Max(f64::NEG_INFINITY)]
    );
}

#[test]
fn the_response_carries_the_aggregations_after_the_hits() {
    let shard = shard("aggs-response");
    let source = source(r#"{"size":1,"sort":[{"n":"asc"}],"aggs":{"lo":{"min":{"field":"n"}}}}"#);
    let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    let mut out = Vec::new();
    crate::response::write_response(&mut out, &shard, "idx", &hits, 3, &mut |_| Ok(()))
        .expect("writes");
    let text = String::from_utf8(out).expect("utf-8");
    assert!(
        text.starts_with(r#"{"took":3,"timed_out":false,"#),
        "{text}"
    );
    assert!(
        text.ends_with(r#"]},"aggregations":{"lo":{"value":0.0}}}"#),
        "{text}"
    );
}

#[test]
fn each_metric_asks_for_what_it_reads() {
    use lucene_aggs::metric_aggs::{NEED_COUNT, NEED_MAX, NEED_MIN, NEED_MIN_MAX, NEED_SUM};

    use super::needs;
    use crate::aggs::metric::Metric;
    assert_eq!(needs(Metric::Min), NEED_MIN);
    assert_eq!(needs(Metric::Max), NEED_MAX);
    assert_eq!(needs(Metric::Sum), NEED_SUM);
    assert_eq!(needs(Metric::Avg), NEED_SUM | NEED_COUNT);
    assert_ne!(needs(Metric::Avg), 0);
    assert_eq!(needs(Metric::Stats), NEED_COUNT | NEED_SUM | NEED_MIN_MAX);
    assert_ne!(needs(Metric::Stats) & NEED_COUNT, 0);
    assert_ne!(needs(Metric::Stats) & NEED_MIN_MAX, 0);
    assert_eq!(needs(Metric::ValueCount), NEED_COUNT);
}
