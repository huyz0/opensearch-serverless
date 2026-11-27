//! Queries, sorts and `search_after` over `float` and `double` fields, on a real index.
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
use oss_testkit::index::{Spec, blobs, doc_double, doc_float};

use super::*;
use crate::dsl::parse_chunks;
use crate::{OpenShard, open_shard};

const DOCS: usize = 200;

fn shard() -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    // Four segments, so a sort merges across them.
    for (key, bytes) in blobs("", &Spec::new("dec", DOCS).segments_of(50).with_decimals()) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "dec", 0).expect("opens")
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Hits, ShardError> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None))
}

fn docs(shard: &OpenShard<Emulator>, body: &str) -> Vec<i32> {
    let hits = run(shard, body).expect("searches");
    let mut docs: Vec<i32> = hits.hits.iter().map(|h| h.doc).collect();
    docs.sort_unstable();
    docs
}

fn refusal(shard: &OpenShard<Emulator>, body: &str) -> String {
    run(shard, body).expect_err("refused").failure.reason
}

#[test]
fn a_term_query_on_a_double_matches_its_documents() {
    let shard = shard();
    // Document i holds i / 4 - 5.
    assert_eq!(docs(&shard, r#"{"query":{"term":{"d":-5}}}"#), [0]);
    assert_eq!(
        docs(&shard, r#"{"query":{"term":{"d":{"value":"2.5"}}}}"#),
        [30]
    );
    assert_eq!(
        docs(&shard, r#"{"query":{"term":{"d":2.6}}}"#),
        [] as [i32; 0]
    );
    assert_eq!(docs(&shard, r#"{"query":{"match":{"d":-4.75}}}"#), [1]);
}

#[test]
fn a_terms_query_on_a_double_matches_any_value() {
    let shard = shard();
    assert_eq!(
        docs(&shard, r#"{"query":{"terms":{"d":[-5,-4.75,99.5]}}}"#),
        [0, 1]
    );
}

#[test]
fn a_range_query_on_a_double_honours_each_bound() {
    let shard = shard();
    let range = |r: &str| docs(&shard, &format!(r#"{{"query":{{"range":{{"d":{r}}}}}}}"#));
    assert_eq!(range(r#"{"gte":0,"lte":0.75}"#), [20, 21, 22, 23]);
    assert_eq!(range(r#"{"gt":0,"lt":1}"#), [21, 22, 23]);
    assert_eq!(range(r#"{"gt":0,"lte":0.25}"#), [21]);
    assert_eq!(range(r#"{"gte":44.5}"#), [198, 199]);
    assert_eq!(range(r#"{"lt":-4.75}"#), [0]);
    assert_eq!(range(r#"{"gt":1,"lt":1}"#), [] as [i32; 0]);
}

#[test]
fn queries_on_a_float_read_the_nearest_float() {
    let shard = shard();
    // Document 1 holds the float nearest 0.1, which the query's 0.1 rounds to.
    assert_eq!(docs(&shard, r#"{"query":{"term":{"f":0.1}}}"#), [1]);
    assert_eq!(
        docs(&shard, r#"{"query":{"terms":{"f":[0,"0.2"]}}}"#),
        [0, 2]
    );
    assert_eq!(
        docs(&shard, r#"{"query":{"range":{"f":{"gte":0.5,"lte":1.0}}}}"#),
        [5, 6, 7, 8, 9, 10]
    );
    // Excluding 0.2 steps one float in, so document 2 is out and 3 is in.
    assert_eq!(
        docs(&shard, r#"{"query":{"range":{"f":{"gt":0.2,"lt":0.4}}}}"#),
        [3]
    );
}

#[test]
fn a_value_that_is_not_a_finite_number_is_refused_as_the_mapper_refuses_it() {
    let shard = shard();
    assert_eq!(
        refusal(&shard, r#"{"query":{"term":{"d":"abc"}}}"#),
        "failed to create query: For input string: \"abc\""
    );
    assert_eq!(
        refusal(&shard, r#"{"query":{"range":{"d":{"lte":"Infinity"}}}}"#),
        "failed to create query: [double] supports only finite values, but got [Infinity]"
    );
    assert_eq!(
        refusal(&shard, r#"{"query":{"term":{"f":1e39}}}"#),
        "failed to create query: [float] supports only finite values, but got [Infinity]"
    );
}

#[test]
fn a_double_sort_orders_across_segments_and_reads_back_doubles() {
    let shard = shard();
    let hits = run(&shard, r#"{"size":3,"sort":[{"d":"desc"}]}"#).expect("searches");
    let got: Vec<(i32, SortValue)> = hits
        .hits
        .iter()
        .map(|h| (h.doc, h.sort[0].clone()))
        .collect();
    let want: Vec<(i32, SortValue)> = (197..200)
        .rev()
        .map(|i| {
            (
                i32::try_from(i).expect("fits"),
                SortValue::Double(doc_double(i)),
            )
        })
        .collect();
    assert_eq!(got, want);
    let hits = run(&shard, r#"{"size":2,"sort":[{"d":"asc"}]}"#).expect("searches");
    let got: Vec<i32> = hits.hits.iter().map(|h| h.doc).collect();
    assert_eq!(got, [0, 1]);
}

#[test]
fn a_float_sort_reads_back_floats() {
    let shard = shard();
    let hits = run(&shard, r#"{"size":2,"sort":[{"f":"asc"}]}"#).expect("searches");
    let got: Vec<(i32, SortValue)> = hits
        .hits
        .iter()
        .map(|h| (h.doc, h.sort[0].clone()))
        .collect();
    assert_eq!(
        got,
        [
            (0, SortValue::Float(0.0)),
            (1, SortValue::Float(doc_float(1)))
        ]
    );
}

#[test]
fn search_after_resumes_a_double_and_a_float_sort() {
    let shard = shard();
    let hits = run(
        &shard,
        r#"{"size":2,"sort":[{"d":"asc"}],"search_after":[-4.5]}"#,
    )
    .expect("searches");
    assert_eq!(hits.hits.iter().map(|h| h.doc).collect::<Vec<_>>(), [3, 4]);
    let hits = run(
        &shard,
        r#"{"size":2,"sort":[{"f":"asc"}],"search_after":["0.2"]}"#,
    )
    .expect("searches");
    assert_eq!(hits.hits.iter().map(|h| h.doc).collect::<Vec<_>>(), [3, 4]);
    assert_eq!(
        refusal(&shard, r#"{"sort":[{"d":"asc"}],"search_after":["x"]}"#).as_str(),
        "Failed to parse search_after value for field [d]."
    );
}

#[test]
fn a_decimal_sort_puts_a_missing_value_last_in_either_direction() {
    use crate::search_sort::{KeyKind, field_sort};
    use lucene_util::numeric_utils::{double_to_sortable_long, float_to_sortable_int};
    let shard = shard();
    let (asc, kind) = field_sort("d", false, &shard, "dec").expect("sorts");
    assert_eq!(kind, KeyKind::Double);
    assert_eq!(asc.missing, double_to_sortable_long(f64::INFINITY));
    let (desc, _) = field_sort("d", true, &shard, "dec").expect("sorts");
    assert_eq!(desc.missing, double_to_sortable_long(f64::NEG_INFINITY));
    let (asc, kind) = field_sort("f", false, &shard, "dec").expect("sorts");
    assert_eq!(kind, KeyKind::Float);
    assert_eq!(asc.missing, i64::from(float_to_sortable_int(f32::INFINITY)));
    let (desc, _) = field_sort("f", true, &shard, "dec").expect("sorts");
    assert_eq!(
        desc.missing,
        i64::from(float_to_sortable_int(f32::NEG_INFINITY))
    );
}

/// The hit's JSON as the response writes it.
fn response(shard: &OpenShard<Emulator>, body: &str) -> String {
    let hits = run(shard, body).expect("searches");
    let mut out = Vec::new();
    crate::response::write_response(&mut out, shard, "idx", "node", &hits, 3, &mut |_| Ok(()))
        .expect("writes");
    String::from_utf8(out).expect("utf-8")
}

#[test]
fn docvalue_fields_write_a_double_and_a_float_widened_to_a_double() {
    let shard = shard();
    let text = response(
        &shard,
        r#"{"query":{"term":{"d":-4.75}},"docvalue_fields":["d","f"]}"#,
    );
    // Document 1 holds -4.75 and the float nearest 0.1, which a double shows in full.
    assert!(
        text.contains(r#""fields":{"d":[-4.75],"f":[0.10000000149011612]}"#),
        "{text}"
    );
}

#[test]
fn the_fields_api_reads_a_double_and_a_float_from_the_source() {
    let shard = shard();
    let text = response(
        &shard,
        r#"{"query":{"term":{"d":-4.75}},"fields":["d","f"],"_source":false}"#,
    );
    assert!(
        text.contains(r#""fields":{"d":[-4.75],"f":[0.1]}"#),
        "{text}"
    );
}
