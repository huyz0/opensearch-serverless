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

pub(super) fn source(body: &str) -> crate::dsl::SearchSource {
    parse_chunks([body.as_bytes()]).expect("parses")
}

pub(super) const DOCS: usize = 200;

fn shard(name: &str) -> OpenShard<Emulator> {
    shard_of(Spec::new(name, DOCS).segments_of(50).with_texts(1))
}

pub(super) fn shard_of(spec: Spec) -> OpenShard<Emulator> {
    let name = spec.name.clone();
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    // Four segments, so each state carries across segments.
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", &name, 0).expect("opens")
}

pub(super) fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Vec<Partial>, ShardError> {
    let source = source(body);
    search(shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
}

pub(super) fn values(shard: &OpenShard<Emulator>, body: &str) -> Vec<Value> {
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
    shard.meta.mapping = std::sync::Arc::new(
        crate::SharedMapping::from_json(
            r#"{"properties":{"n":{"type":"long","doc_values":false},"p":{"type":"ip"},
            "b":{"type":"byte"}}}"#,
        )
        .expect("mapping"),
    );
    let refused = |body: &str| run(&shard, body).expect_err("refused");
    let e = refused(r#"{"aggs":{"a":{"sum":{"field":"n"}}}}"#);
    assert_eq!(e.status, 400);
    assert_eq!(
        e.failure.reason,
        "Can't load fielddata on [n] because fielddata is unsupported on fields of type [long]. \
         Use doc values instead."
    );
    let e = refused(r#"{"aggs":{"a":{"avg":{"field":"p"}}}}"#);
    assert_eq!(e.status, 400);
    assert!(
        e.failure
            .reason
            .ends_with("[p] is not supported by this node yet"),
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
    crate::response::write_response(&mut out, &shard, "idx", "node", &hits, 3, &mut |_| Ok(()))
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

fn cardinality(shard: &OpenShard<Emulator>, body: &str) -> u64 {
    match values(shard, body).as_slice() {
        [Value::Cardinality(sketch)] => sketch.as_ref().map_or(0, |s| s.cardinality()),
        other => panic!("{body}: {other:?}"),
    }
}

#[test]
fn cardinality_estimates_the_distinct_values_as_opensearch_does() {
    let shard = shard("aggs-cardinality");
    // `n` is 0 to 199: the sketch of their `mix64` hashes, whose estimates
    // OpenSearch's `HyperLogLogPlusPlus` prints as 200 (precision 14,
    // counting exactly), 206 (10) and 168 (4).
    let body = |extra: &str| {
        format!(r#"{{"size":0,"aggs":{{"c":{{"cardinality":{{"field":"n"{extra}}}}}}}}}"#)
    };
    assert_eq!(cardinality(&shard, &body("")), 200);
    assert_eq!(
        cardinality(&shard, &body(r#","precision_threshold":100"#)),
        206
    );
    assert_eq!(
        cardinality(&shard, &body(r#","precision_threshold":0"#)),
        168
    );
    assert_eq!(
        cardinality(&shard, &body(r#","execution_hint":"Ordinals""#)),
        200
    );
    // Only the query's matches: one document.
    let one = r#"{"size":0,"query":{"term":{"tag":"t3"}},"aggs":{"c":{"cardinality":{"field":"n","precision_threshold":0}}}}"#;
    assert!(cardinality(&shard, one) > 0);
    let none =
        r#"{"size":0,"query":{"term":{"tag":"nope"}},"aggs":{"c":{"cardinality":{"field":"n"}}}}"#;
    assert_eq!(values(&shard, none), [Value::Cardinality(None)]);
    let unmapped = r#"{"size":0,"aggs":{"c":{"cardinality":{"field":"nope"}}}}"#;
    assert_eq!(values(&shard, unmapped), [Value::Cardinality(None)]);
    // Beside other metrics, in one pass.
    let both = r#"{"size":0,"aggs":{"m":{"max":{"field":"n"}},"c":{"cardinality":{"field":"n"}}}}"#;
    match values(&shard, both).as_slice() {
        [Value::Max(m), Value::Cardinality(Some(s))] => {
            assert_eq!(*m, 199.0);
            assert_eq!(s.cardinality(), 200);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn cardinality_options_the_shard_refuses_fail_it_in_opensearch_order() {
    let shard = shard("aggs-cardinality-refused");
    for (body, want) in [
        (
            r#"{"aggs":{"c":{"cardinality":{"field":"n","execution_hint":"foo"}}}}"#,
            "Unknown execution_hint: [foo], expected any of [direct, ordinals]",
        ),
        (
            r#"{"aggs":{"c":{"cardinality":{"field":"nope","precision_threshold":9223372036854775807}}}}"#,
            "maxValue must be non-negative (got: -4)",
        ),
        // Every factory is built before any aggregator: the second's hint
        // fails before the first's precision.
        (
            r#"{"aggs":{"a":{"cardinality":{"field":"n","precision_threshold":9223372036854775807}},"b":{"cardinality":{"field":"n","execution_hint":"x"}}}}"#,
            "Unknown execution_hint: [x], expected any of [direct, ordinals]",
        ),
        // The field first, then the hint.
        (
            r#"{"aggs":{"c":{"cardinality":{"field":"x0","execution_hint":"x"}}}}"#,
            "Text fields are not optimised for operations that require per-document field data \
             like aggregations and sorting, so these operations are disabled by default. Please \
             use a keyword field instead. Alternatively, set fielddata=true on [x0] in order to \
             load field data by uninverting the inverted index. Note that this can use \
             significant memory.",
        ),
        (
            r#"{"aggs":{"c":{"value_count":{"field":"tag"}}}}"#,
            "[value_count] of the keyword field [tag] is not supported by this node yet",
        ),
    ] {
        let e = run(&shard, body).expect_err("refused");
        assert_eq!(e.status, 400, "{body}");
        assert_eq!(e.failure.reason, want, "{body}");
    }
}

#[test]
fn a_cardinality_reserves_its_sketch() {
    let shard = shard("aggs-cardinality-bytes");
    let min = source(r#"{"size":0,"aggs":{"c":{"min":{"field":"n"}}}}"#);
    let card = source(r#"{"size":0,"aggs":{"c":{"cardinality":{"field":"n"}}}}"#);
    let low =
        source(r#"{"size":0,"aggs":{"c":{"cardinality":{"field":"n","precision_threshold":0}}}}"#);
    let bytes = |s: &crate::dsl::SearchSource| crate::search::hits_bytes(s, &shard);
    assert_eq!(
        bytes(&card) - bytes(&min),
        crate::aggs::metric::hll::sketch_bytes(14)
    );
    assert_eq!(
        bytes(&low) - bytes(&min),
        crate::aggs::metric::hll::sketch_bytes(4)
    );
}

/// The values a percentiles result writes, keyed by its keys.
fn percentiles(shard: &OpenShard<Emulator>, body: &str) -> Vec<(f64, Option<f64>)> {
    match values(shard, body).as_slice() {
        [
            Value::Percentiles {
                ranks,
                keys,
                digest,
                ..
            },
        ] => keys
            .iter()
            .map(|&k| {
                let v = digest.as_ref().filter(|d| d.size() > 0).map(|d| {
                    if *ranks {
                        d.cdf(k).clamp(0.0, 1.0) * 100.0
                    } else {
                        d.quantile(k / 100.0)
                    }
                });
                (k, v)
            })
            .collect(),
        other => panic!("{body}: {other:?}"),
    }
}

#[test]
fn percentiles_follow_the_digest_opensearch_builds() {
    let shard = shard("aggs-percentiles");
    // `n` is 0 to 199, added in document order across the four segments;
    // OpenSearch's `TDigestState` answers these.
    let all = percentiles(
        &shard,
        r#"{"size":0,"aggs":{"p":{"percentiles":{"field":"n"}}}}"#,
    );
    let want = [2.0, 10.0, 49.5, 99.5, 149.5, 190.0, 198.0];
    let keys = [1.0, 5.0, 25.0, 50.0, 75.0, 95.0, 99.0];
    assert_eq!(
        all,
        keys.iter()
            .zip(want)
            .map(|(&k, v)| (k, Some(v)))
            .collect::<Vec<_>>()
    );
    let small = percentiles(
        &shard,
        r#"{"size":0,"aggs":{"p":{"percentiles":{"field":"n","percents":[1],"tdigest":{"compression":5}}}}}"#,
    );
    assert_eq!(small, [(1.0, Some(1.0))]);
    let ranks = percentiles(
        &shard,
        r#"{"size":0,"aggs":{"r":{"percentile_ranks":{"field":"n","values":[-1,50.5,100,199,1e9]}}}}"#,
    );
    assert_eq!(
        ranks,
        [
            (-1.0, Some(0.0)),
            (50.5, Some(25.5)),
            (100.0, Some(50.24999999999999)),
            (199.0, Some(99.75)),
            (1e9, Some(100.0)),
        ]
    );
    // Only the query's matches, 10 to 19.
    let some = r#"{"size":0,"query":{"range":{"n":{"gte":10,"lt":20}}},
        "aggs":{"p":{"percentiles":{"field":"n","percents":[50]}}}}"#;
    assert_eq!(percentiles(&shard, some), [(50.0, Some(15.0))]);
    let none = r#"{"size":0,"query":{"term":{"tag":"nope"}},
        "aggs":{"p":{"percentile_ranks":{"field":"n","values":[1]}}}}"#;
    assert_eq!(percentiles(&shard, none), [(1.0, None)]);
    let unmapped = r#"{"size":0,"aggs":{"p":{"percentiles":{"field":"nope","percents":[5]}}}}"#;
    assert_eq!(percentiles(&shard, unmapped), [(5.0, None)]);
    let e = run(&shard, r#"{"aggs":{"p":{"percentiles":{"field":"tag"}}}}"#).expect_err("refused");
    assert_eq!(
        e.failure.reason,
        "Field [tag] of type [keyword] is not supported for aggregation [percentiles]"
    );
}

#[test]
fn a_percentiles_reserves_its_digest_and_keys() {
    let shard = shard("aggs-percentiles-bytes");
    let min = source(r#"{"size":0,"aggs":{"c":{"min":{"field":"n"}}}}"#);
    let p = source(r#"{"size":0,"aggs":{"c":{"percentiles":{"field":"n","percents":[1,2]}}}}"#);
    let big = source(
        r#"{"size":0,"aggs":{"c":{"percentile_ranks":{"field":"n","values":[1],"tdigest":{"compression":1000}}}}}"#,
    );
    let bytes = |s: &crate::dsl::SearchSource| crate::search::hits_bytes(s, &shard);
    let digest = crate::aggs::metric::tdigest::digest_bytes;
    assert_eq!(bytes(&p) - bytes(&min), digest(100.0) + 2 * 16);
    assert_eq!(bytes(&big) - bytes(&min), digest(1000.0) + 16);
}

#[test]
fn a_keyword_cardinality_counts_the_distinct_terms_across_segments() {
    let shard = shard("aggs-keyword-cardinality");
    // `tag` is `t<i % 16>`: sixteen terms, each segment holding all of them.
    let body = |extra: &str, query: &str| {
        format!(r#"{{"size":0{query},"aggs":{{"c":{{"cardinality":{{"field":"tag"{extra}}}}}}}}}"#)
    };
    assert_eq!(cardinality(&shard, &body("", "")), 16);
    assert_eq!(
        cardinality(&shard, &body(r#","execution_hint":"direct""#, "")),
        16
    );
    // Only the matches' terms: `n` below 5 are five terms.
    let five = body("", r#","query":{"range":{"n":{"lt":5}}}"#);
    assert_eq!(cardinality(&shard, &five), 5);
    let none = body("", r#","query":{"term":{"tag":"nope"}}"#);
    assert_eq!(values(&shard, &none), [Value::Cardinality(None)]);
}

#[test]
fn a_keyword_cardinality_reserves_its_ordinals_and_sketch() {
    let shard = shard("aggs-keyword-cardinality-reserved");
    let with = source(r#"{"size":0,"aggs":{"c":{"cardinality":{"field":"tag"}}}}"#);
    let without = source(r#"{"size":0,"aggs":{"c":{"cardinality":{"field":"n"}}}}"#);
    let (a, b) = (
        crate::search::hits_bytes(&with, &shard),
        crate::search::hits_bytes(&without, &shard),
    );
    let (terms, _, _) = crate::aggs::bucket::keyword_collect::dictionary(&shard, "tag");
    assert!(terms > 0);
    assert!(a - b >= terms * crate::aggs::bucket::keyword_collect::ORD_BYTES);
}
