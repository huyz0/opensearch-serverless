//! `adjacency_matrix`: its body against the recorded Java, its counts on a real
//! index and its result as written.
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

use super::filters::{FilterKind, FiltersResult};
use super::filters_render::write_filters;
use crate::aggs::partial::Value;
use crate::dsl::parse_chunks;
use crate::search::search;
use crate::{OpenShard, open_shard};

const RECORDED: &str =
    include_str!("../../../../../fixtures/adjacency-agg/java-adjacency-3.10.0.txt");

fn request(body: &str) -> String {
    format!(r#"{{"aggs":{{"p":{{"adjacency_matrix":{body}}}}}}}"#)
}

fn kind_of(class: &str) -> &'static str {
    match class {
        "XContentParseException" => "x_content_parse_exception",
        "IllegalArgumentException" => "illegal_argument_exception",
        "IllegalStateException" => "illegal_state_exception",
        "ParsingException" => "parsing_exception",
        other => panic!("unknown class {other}"),
    }
}

/// The recorder parsed the body alone: a location in a message is where the
/// body starts there, not in the whole request.
fn unlocated(m: &str) -> String {
    if let Some((head, _)) = m.split_once(" found at [") {
        return head.to_owned();
    }
    match m.strip_prefix('[').and_then(|r| r.split_once("] ")) {
        Some((at, rest)) if at.contains(':') => rest.to_owned(),
        _ => m.to_owned(),
    }
}

// AC-1, AC-2
#[test]
fn a_body_is_read_as_java_reads_it() {
    let (mut ok, mut failed) = (0, 0);
    for line in RECORDED.lines().filter_map(|l| l.strip_prefix("parse ")) {
        let (body, outcome) = line.split_once(" -> ").expect("outcome");
        let got = parse_chunks([request(body).as_bytes()])
            .map(|_| ())
            .map_err(|e| e.into_failure());
        match outcome.strip_prefix("ok ") {
            Some(_) => {
                got.unwrap_or_else(|f| panic!("{line}: {}", f.reason));
                ok += 1;
            }
            None => {
                let mut at = Some(got.expect_err(line));
                for link in outcome.split(" <- ") {
                    let (class, message) = link.split_once(": ").expect("message");
                    let held = at.take().expect("a link");
                    let class = class.rsplit('.').next().expect("class");
                    assert_eq!(held.kind, kind_of(class), "{line}");
                    assert_eq!(unlocated(&held.reason), unlocated(message), "{line}");
                    at = held.cause.map(|c| *c);
                }
                failed += 1;
            }
        }
    }
    assert!((ok, failed) == (9, 14), "{ok} {failed}");
}

fn shard() -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    let name = "adjacency";
    for (key, bytes) in blobs("", &Spec::new(name, 200).segments_of(50).with_texts(1)) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", name, 0).expect("opens")
}

fn counts(shard: &OpenShard<Emulator>, options: &str) -> Vec<(String, u64)> {
    let body = r#"{"size":0,"aggs":{"p":{"adjacency_matrix":{OPTIONS"filters":{
            "low":{"range":{"n":{"lt":10}}},
            "t0":{"term":{"tag":"t0"}},
            "all":{"match_all":{}}}}}}}"#
        .replace("OPTIONS", options);
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let mut parts = search(shard, "idx", &source, &Analyzer::standard(None))
        .expect("searches")
        .aggregations;
    match parts.remove(0).value {
        Value::Filters(r) => r.buckets,
        other => panic!("not a filters: {other:?}"),
    }
}

fn pairs(v: &[(&str, u64)]) -> Vec<(String, u64)> {
    v.iter().map(|(k, n)| ((*k).to_owned(), *n)).collect()
}

// AC-3
#[test]
fn each_filter_and_each_pair_of_filters_counts_the_documents_it_matches() {
    let shard = shard();
    // `n` is each document's own number 0 to 199; `tag` cycles over 16.
    assert_eq!(
        counts(&shard, ""),
        pairs(&[
            ("all", 200),
            ("all&low", 10),
            ("all&t0", 13),
            ("low", 10),
            ("low&t0", 1),
            ("t0", 13),
        ])
    );
}

// AC-4
#[test]
fn a_separator_names_the_pairs_and_only_intersections_leaves_out_the_filters() {
    let shard = shard();
    assert_eq!(
        counts(&shard, r#""separator":"|","show_only_intersecting":true,"#),
        pairs(&[("all|low", 10), ("all|t0", 13), ("low|t0", 1)])
    );
}

fn written(buckets: &[(&str, u64)]) -> String {
    let result = FiltersResult {
        kind: FilterKind::AdjacencyMatrix,
        buckets: pairs(buckets),
        ..FiltersResult::default()
    };
    let mut out = Vec::new();
    write_filters(&mut out, &result).expect("writes");
    String::from_utf8(out).expect("utf8")
}

// AC-5
#[test]
fn only_the_buckets_with_documents_are_written() {
    assert_eq!(
        written(&[("a", 2), ("a&b", 0), ("b", 3)]),
        r#""buckets":[{"key":"a","doc_count":2},{"key":"b","doc_count":3}]"#
    );
}

// AC-7
#[test]
fn a_sub_aggregation_is_refused() {
    let body = r#"{"aggs":{"p":{"adjacency_matrix":{"filters":{"a":{"match_all":{}}}},"aggs":{"s":{"sum":{"field":"n"}}}}}}"#;
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        failure.reason.contains("cannot accept sub-aggregations"),
        "{}",
        failure.reason
    );
}
