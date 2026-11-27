//! `auto_date_histogram`: its body and its reduce against the recorded Java,
//! its shard on a real index and its result as written.
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

use super::auto_date::{AutoDateResult, AutoDateSpec, reduce_results};
use super::auto_date_render::write_auto_date;
use crate::aggs::partial::{Format, Value};
use crate::date_pattern::DatePattern;
use crate::dsl::parse_chunks;
use crate::search::search;
use crate::{OpenShard, open_shard};

const RECORDED: &str =
    include_str!("../../../../../fixtures/auto-date-histogram/java-auto-date-3.10.0.txt");

const DAY: i64 = 86_400_000;
const HOUR: i64 = 3_600_000;
const T0: i64 = 1_700_000_000_000 - (1_700_000_000_000 % DAY);

fn request(body: &str) -> String {
    format!(r#"{{"aggs":{{"p":{{"auto_date_histogram":{body}}}}}}}"#)
}

fn kind_of(class: &str) -> &'static str {
    match class {
        "XContentParseException" => "x_content_parse_exception",
        "IllegalArgumentException" => "illegal_argument_exception",
        other => panic!("unknown class {other}"),
    }
}

/// The recorder parsed the body alone: a location in a message is where the
/// body starts there, not in the whole request.
fn unlocated(m: &str) -> String {
    match m.strip_prefix('[').and_then(|r| r.split_once("] ")) {
        Some((at, rest)) if at.contains(':') => rest.to_owned(),
        _ => m.to_owned(),
    }
}

// AC-1
#[test]
fn a_body_is_read_as_java_reads_it() {
    let mut seen = 0;
    for line in RECORDED.lines().filter_map(|l| l.strip_prefix("parse ")) {
        let (body, outcome) = line.split_once(" -> ").expect("outcome");
        let got = parse_chunks([request(body).as_bytes()])
            .map(|_| ())
            .map_err(|e| e.into_failure());
        seen += 1;
        if outcome.starts_with("ok ") {
            // Options the node does not serve yet are refused by name.
            if let Err(f) = got {
                assert!(
                    f.reason.contains("not supported by this node yet"),
                    "{line}: {}",
                    f.reason
                );
            }
            continue;
        }
        let mut at = Some(got.expect_err(line));
        for link in outcome.split(" <- ") {
            let (class, message) = link.split_once(": ").expect("message");
            let held = at.take().expect("a link");
            let class = class.rsplit('.').next().expect("class");
            assert_eq!(held.kind, kind_of(class), "{line}");
            assert_eq!(
                unlocated(&held.reason).trim_end(),
                unlocated(message).trim_end(),
                "{line}"
            );
            at = held.cause.map(|c| *c);
        }
    }
    assert_eq!(seen, 33);
}

fn spec(minimum: Option<&str>, target: usize) -> AutoDateSpec {
    let base = match minimum {
        None | Some("second") => 0,
        Some("minute") => 1,
        Some("hour") => 2,
        other => panic!("{other:?}"),
    };
    AutoDateSpec {
        target,
        base,
        zone_ms: 0,
        region: None,
        format: DatePattern::parse("strict_date_optional_time"),
    }
}

/// A shard's result: the rounding index (in the list from the minimum), the
/// inner interval and `(offset in step milliseconds, count)` pairs.
fn shard(
    minimum: Option<&str>,
    target: usize,
    idx: usize,
    step: i64,
    pairs: &[(i64, u64)],
) -> AutoDateResult {
    AutoDateResult {
        spec: spec(minimum, target),
        idx,
        inner: 1,
        buckets: pairs.iter().map(|&(n, c)| (T0 + n * step, c)).collect(),
        reduced: false,
    }
}

fn written(parts: &[&AutoDateResult]) -> String {
    let reduced = reduce_results(parts).expect("reduces");
    let mut out = Vec::new();
    write_auto_date(&mut out, Format::Date, &reduced).expect("writes");
    format!("{{\"p\":{{{}}}}}", String::from_utf8(out).expect("utf8"))
}

fn recorded(name: &str) -> &'static str {
    RECORDED
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{name} ")))
        .unwrap_or_else(|| panic!("no recording {name}"))
}

// AC-2
#[test]
fn the_final_reduce_gives_the_bytes_java_wrote() {
    let thirty: Vec<(i64, u64)> = (0..30).map(|i| (i, i as u64 + 1)).collect();
    let hours48: Vec<(i64, u64)> = (0..48).map(|k| (k, 1)).collect();
    let cases: Vec<(&str, String)> = vec![
        (
            "reduce.days5",
            written(&[&shard(
                None,
                10,
                3,
                DAY,
                &[(0, 2), (1, 3), (2, 1), (4, 5), (5, 1)],
            )]),
        ),
        (
            "reduce.days30.t10",
            written(&[&shard(None, 10, 3, DAY, &thirty)]),
        ),
        (
            "reduce.days30.t3",
            written(&[&shard(None, 3, 3, DAY, &thirty)]),
        ),
        (
            "reduce.days30.t100",
            written(&[&shard(None, 100, 3, DAY, &thirty)]),
        ),
        (
            "reduce.days30.t30",
            written(&[&shard(None, 30, 3, DAY, &thirty)]),
        ),
        (
            "reduce.days30.t29",
            written(&[&shard(None, 29, 3, DAY, &thirty)]),
        ),
        (
            "reduce.mixed",
            written(&[
                &shard(None, 10, 2, HOUR, &[(0, 1), (1, 2), (5, 3)]),
                &shard(None, 10, 3, DAY, &[(0, 4), (2, 6)]),
            ]),
        ),
        (
            "reduce.overlap",
            written(&[
                &shard(None, 10, 3, DAY, &[(0, 1), (1, 2)]),
                &shard(None, 10, 3, DAY, &[(1, 3), (3, 4)]),
            ]),
        ),
        ("reduce.empty", written(&[&shard(None, 10, 3, DAY, &[])])),
        (
            "reduce.hours48",
            written(&[&shard(None, 10, 2, HOUR, &hours48)]),
        ),
        (
            "reduce.inner3",
            written(&[&shard(None, 10, 2, HOUR, &[(0, 1), (1, 2), (2, 3)])]),
        ),
        (
            "reduce.minimum.hour",
            written(&[&shard(Some("hour"), 10, 0, HOUR, &[(0, 1), (3, 2)])]),
        ),
    ];
    for (name, got) in cases {
        assert_eq!(got, recorded(name), "{name}");
    }
}

// AC-4
#[test]
fn typed_keys_name_the_result_by_its_type() {
    let value = Value::AutoDateHistogram(Box::new(shard(None, 10, 3, DAY, &[(0, 1)])));
    let mut out = Vec::new();
    crate::aggs::metric::typed::scoped(true, || {
        crate::aggs::metric::typed::write_key(&mut out, "p", &value, Format::Date)
    })
    .expect("writes");
    assert_eq!(
        String::from_utf8(out).expect("utf8"),
        "\"auto_date_histogram#p\""
    );
}

fn index(name: &str) -> OpenShard<Emulator> {
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

fn auto(shard: &OpenShard<Emulator>, body: &str) -> AutoDateResult {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let mut parts = search(shard, "idx", &source, &Analyzer::standard(None))
        .expect("searches")
        .aggregations;
    match parts.remove(0).value {
        Value::AutoDateHistogram(r) => *r,
        other => panic!("not an auto_date_histogram: {other:?}"),
    }
}

// AC-3
#[test]
fn a_shard_counts_the_documents_in_the_rounding_its_data_fits() {
    // `n` is each document's own number 0 to 199, read as epoch milliseconds:
    // all in one second.
    let shard = index("auto-date");
    let got = auto(
        &shard,
        r#"{"size":0,"aggs":{"p":{"auto_date_histogram":{"field":"n"}}}}"#,
    );
    assert_eq!((got.idx, got.buckets), (0, vec![(0, 200)]));
    // `minimum_interval` starts the list at the hour.
    let got = auto(
        &shard,
        r#"{"aggs":{"p":{"auto_date_histogram":{"field":"n","minimum_interval":"hour"}}}}"#,
    );
    assert_eq!((got.idx, got.buckets), (0, vec![(0, 200)]));
    // A query that matches nothing has no bucket.
    let got = auto(
        &shard,
        r#"{"query":{"term":{"tag":"none"}},"aggs":{"p":{"auto_date_histogram":{"field":"n"}}}}"#,
    );
    assert!(got.buckets.is_empty());
}

// AC-6
#[test]
fn a_sub_aggregation_is_refused() {
    let body = r#"{"aggs":{"p":{"auto_date_histogram":{"field":"n"},"aggs":{"s":{"sum":{"field":"n"}}}}}}"#;
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        failure.reason.contains("cannot accept sub-aggregations"),
        "{}",
        failure.reason
    );
}
