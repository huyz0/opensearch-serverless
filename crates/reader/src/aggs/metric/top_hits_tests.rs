//! `top_hits` parsed, checked and run on a real index of four segments.
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

use crate::aggs::metric::top_hits::{DEFAULT_SIZE, MAX_INNER_RESULT_WINDOW};
use crate::aggs::metric::{Metric, TopHitsResult, TopHitsSpec, Value};
use crate::dsl::{DslError, SearchSource, parse_chunks};
use crate::search::{ShardError, SortValue, hits_bytes, search};
use crate::{OpenShard, open_shard};

const DOCS: usize = 200;

fn source(body: &str) -> Result<SearchSource, DslError> {
    parse_chunks([body.as_bytes()])
}

fn spec_of(body: &str) -> TopHitsSpec {
    let s = source(&format!(r#"{{"aggs":{{"t":{{"top_hits":{body}}}}}}}"#)).expect("parses");
    *s.aggs
        .into_iter()
        .next()
        .and_then(|a| a.top_hits)
        .expect("a top_hits")
}

fn refused(body: &str) -> String {
    source(&format!(r#"{{"aggs":{{"t":{{"top_hits":{body}}}}}}}"#))
        .expect_err("refused")
        .into_failure()
        .reason
        .clone()
}

fn shard(name: &str) -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    // Four segments, so the hits merge across segments.
    for (key, bytes) in blobs("", &Spec::new(name, DOCS).segments_of(50).with_texts(1)) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", name, 0).expect("opens")
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Vec<Value>, ShardError> {
    let source = source(body).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None))
        .map(|h| h.aggregations.into_iter().map(|p| p.value).collect())
}

fn top(shard: &OpenShard<Emulator>, body: &str) -> TopHitsResult {
    match run(shard, body).expect("searches").into_iter().next() {
        Some(Value::TopHits(t)) => *t,
        other => panic!("not a top_hits: {other:?}"),
    }
}

fn docs(t: &TopHitsResult) -> Vec<i32> {
    t.hits.iter().map(|h| h.doc).collect()
}

fn failure(shard: &OpenShard<Emulator>, body: &str) -> (u16, String) {
    let e = run(shard, body).expect_err("fails");
    (e.status, e.failure.reason.clone())
}

#[test]
fn the_defaults_are_the_first_three_of_the_score_order() {
    let s = spec_of("{}");
    assert_eq!(s, TopHitsSpec::default());
    assert_eq!((s.from, s.size, DEFAULT_SIZE), (0, 3, 3));
    assert!(s.sort.is_empty() && s.docvalue_fields.is_empty() && !s.explain);
    assert!(s.source.is_whole() && s.highlight.is_none());
    let source = source(r#"{"aggs":{"t":{"top_hits":{}}}}"#).expect("parses");
    let agg = source.aggs.first().expect("one");
    assert_eq!(agg.metric, Metric::TopHits);
    assert_eq!(Metric::from_name("top_hits"), Some(Metric::TopHits));
    assert!(agg.field.is_empty());
}

#[test]
fn every_option_is_read() {
    let s = spec_of(
        r#"{"from":1,"size":"2","sort":[{"n":"desc"}],"_source":["tag"],
            "docvalue_fields":["n"],"explain":true,"track_scores":false,
            "version":false,"seq_no_primary_term":false,
            "highlight":{"fields":{"tag":{}}}}"#,
    );
    assert_eq!((s.from, s.size), (1, 2));
    assert_eq!(s.sort.len(), 1);
    assert!(!s.source.is_whole());
    assert_eq!(s.docvalue_fields.len(), 1);
    assert!(s.explain && s.highlight.is_some());
    // A sort and a source may be given as a single value.
    assert_eq!(spec_of(r#"{"sort":"n"}"#).sort.len(), 1);
    assert!(spec_of(r#"{"_source":false}"#).source.skip);
    assert_eq!(spec_of(r#"{"sort":{"n":"asc"}}"#).sort.len(), 1);
}

#[test]
fn bad_options_fail_as_opensearch_fails_them() {
    assert_eq!(
        refused(r#"{"size":-1}"#),
        "[size] must be greater than or equal to 0. Found [-1] in [t]"
    );
    assert_eq!(
        refused(r#"{"from":-2}"#),
        "[from] must be greater than or equal to 0. Found [-2] in [t]"
    );
    assert_eq!(
        refused(r#"{"bogus":1}"#),
        "Unknown key for a VALUE_NUMBER in [bogus]."
    );
    assert_eq!(
        refused(r#"{"from":null}"#),
        "Unknown key for a VALUE_NULL in [from]."
    );
    assert_eq!(
        refused(r#"{"x":{}}"#),
        "Unknown key for a START_OBJECT in [x]."
    );
    assert_eq!(
        refused(r#"{"x":[]}"#),
        "Unknown key for a START_ARRAY in [x]."
    );
    assert!(refused(r#"{"size":"two"}"#).contains("two"));
    assert!(refused(r#"{"explain":"maybe"}"#).contains("maybe"));
}

#[test]
fn options_this_node_does_not_serve_are_refused_by_name() {
    for (body, key) in [
        (r#"{"track_scores":true}"#, "track_scores"),
        (r#"{"version":true}"#, "version"),
        (r#"{"seq_no_primary_term":true}"#, "seq_no_primary_term"),
        (r#"{"stored_fields":"_none_"}"#, "stored_fields"),
        (r#"{"stored_fields":["n"]}"#, "stored_fields"),
        (r#"{"script_fields":{"x":{}}}"#, "script_fields"),
        (r#"{"fields":["n"]}"#, "fields"),
    ] {
        assert_eq!(
            refused(body),
            format!("the [top_hits] aggregation's [{key}] is not supported by this node yet"),
            "{body}"
        );
    }
}

#[test]
fn a_top_hits_is_one_of_the_aggregations_in_request_order() {
    let shard = shard("th-order");
    let got = run(
        &shard,
        r#"{"aggs":{"a":{"max":{"field":"n"}},"t":{"top_hits":{"size":1}},"c":{"min":{"field":"n"}}}}"#,
    )
    .expect("searches");
    assert_eq!(got.len(), 3);
    assert_eq!(got.first(), Some(&Value::Max(199.0)));
    assert!(matches!(got.get(1), Some(Value::TopHits(t)) if t.hits.len() == 1));
    assert_eq!(got.get(2), Some(&Value::Min(0.0)));
    assert!(crate::aggs::partial::reduce(&got[1..2]).is_none());
}

#[test]
fn the_first_hits_in_score_order_with_an_exact_total() {
    let shard = shard("th-score");
    let t = top(&shard, r#"{"size":0,"aggs":{"t":{"top_hits":{}}}}"#);
    assert_eq!(t.total, DOCS as u64);
    assert_eq!(t.max_score, Some(1.0));
    assert_eq!(docs(&t), [0, 1, 2]);
    assert!(
        t.hits
            .iter()
            .all(|h| h.score == Some(1.0) && h.sort.is_empty())
    );
    // `from` skips hits, but the best score is the window's first.
    let t = top(&shard, r#"{"aggs":{"t":{"top_hits":{"from":2,"size":2}}}}"#);
    assert_eq!(docs(&t), [2, 3]);
    assert_eq!(t.max_score, Some(1.0));
    // Past the matches: no hits, and the best score is still reported.
    let t = top(
        &shard,
        r#"{"query":{"range":{"n":{"gte":10,"lt":20}}},"aggs":{"t":{"top_hits":{"from":50,"size":5}}}}"#,
    );
    assert!(t.hits.is_empty());
    assert_eq!((t.total, t.max_score), (10, Some(1.0)));
}

#[test]
fn the_request_s_own_sort_paging_and_total_do_not_reach_it() {
    let shard = shard("th-own");
    let t = top(
        &shard,
        r#"{"size":0,"from":5,"sort":[{"n":"desc"}],"track_total_hits":2,
            "aggs":{"t":{"top_hits":{"size":2}}}}"#,
    );
    assert_eq!(docs(&t), [0, 1]);
    assert_eq!(t.total, DOCS as u64);
    let t = top(
        &shard,
        r#"{"track_total_hits":false,"aggs":{"t":{"top_hits":{"size":1}}}}"#,
    );
    assert_eq!(t.total, DOCS as u64);
}

#[test]
fn a_sort_of_its_own_orders_across_segments_and_reports_no_score() {
    let shard = shard("th-sort");
    let t = top(
        &shard,
        r#"{"aggs":{"t":{"top_hits":{"sort":[{"n":"desc"}],"from":1,"size":3}}}}"#,
    );
    assert_eq!(docs(&t), [198, 197, 196]);
    assert_eq!(t.max_score, None);
    assert!(t.hits.iter().all(|h| h.score.is_none()));
    assert_eq!(
        t.hits.first().map(|h| h.sort.clone()),
        Some(vec![SortValue::Long(198)])
    );
    // A keyword sort carries the term.
    let t = top(
        &shard,
        r#"{"query":{"term":{"tag":"t3"}},"aggs":{"t":{"top_hits":{"sort":[{"tag":"asc"},{"n":"desc"}],"size":2}}}}"#,
    );
    assert_eq!(docs(&t), [195, 179]);
    assert_eq!(
        t.hits.first().map(|h| h.sort.first().cloned()),
        Some(Some(SortValue::Term(Some(b"t3".to_vec()))))
    );
    assert_eq!(t.total, 13);
    // Sorting by the score too leaves the hit's own score null.
    let t = top(
        &shard,
        r#"{"aggs":{"t":{"top_hits":{"sort":[{"tag":"desc"},"_score"],"size":2}}}}"#,
    );
    assert!(t.hits.iter().all(|h| h.score.is_none()));
    assert!(matches!(
        t.hits.first().and_then(|h| h.sort.get(1)),
        Some(SortValue::Score(s)) if *s == 1.0
    ));
    assert_eq!(t.max_score, None);
}

#[test]
fn hits_are_the_query_s_matches_only() {
    let shard = shard("th-query");
    let t = top(
        &shard,
        r#"{"query":{"range":{"n":{"gte":10,"lt":20}}},"aggs":{"t":{"top_hits":{"size":4}}}}"#,
    );
    assert_eq!(docs(&t), [10, 11, 12, 13]);
    assert_eq!(t.total, 10);
    let none = top(
        &shard,
        r#"{"query":{"term":{"tag":"nothing"}},"aggs":{"t":{"top_hits":{}}}}"#,
    );
    assert_eq!(none, TopHitsResult::empty());
}

#[test]
fn the_window_a_size_and_a_sort_are_checked_as_they_are_there() {
    let shard = shard("th-errors");
    let window = |from: usize, size: usize| {
        format!(
            "Top hits result window is too large, the top hits aggregator [t]'s from + size must \
             be less than or equal to: [100] but was [{}]. This limit can be set by changing \
             the [index.max_inner_result_window] index level setting.",
            from + size
        )
    };
    assert_eq!(MAX_INNER_RESULT_WINDOW, 100);
    let body = |opts: &str| format!(r#"{{"aggs":{{"t":{{"top_hits":{opts}}}}}}}"#);
    assert_eq!(
        failure(&shard, &body(r#"{"size":101}"#)),
        (400, window(0, 101))
    );
    assert_eq!(
        failure(&shard, &body(r#"{"from":1,"size":100}"#)),
        (400, window(1, 100))
    );
    // The edge is allowed.
    assert_eq!(top(&shard, &body(r#"{"size":100}"#)).hits.len(), 100);
    assert_eq!(
        failure(&shard, &body(r#"{"size":0}"#)),
        (
            400,
            "numHits must be > 0; please use TotalHitCountCollectorManager if you just need the \
             total hit count"
                .to_owned()
        )
    );
    // The window is checked first.
    assert_eq!(
        failure(&shard, &body(r#"{"from":200,"size":0}"#)),
        (400, window(200, 0))
    );
    assert_eq!(
        failure(&shard, &body(r#"{"sort":[{"nope":"asc"}]}"#)),
        (
            400,
            "No mapping found for [nope] in order to sort on".to_owned()
        )
    );
}

#[test]
fn explain_is_asked_per_hit_and_refused_for_queries_the_engine_cannot_explain() {
    let shard = shard("th-explain");
    assert_eq!(
        failure(
            &shard,
            r#"{"query":{"exists":{"field":"n"}},"aggs":{"t":{"top_hits":{"explain":true}}}}"#
        ),
        (
            400,
            "explain of an exists query is not supported by this node yet".to_owned()
        )
    );
    let t = top(
        &shard,
        r#"{"aggs":{"t":{"top_hits":{"explain":true,"size":2}}}}"#,
    );
    assert_eq!(t.explanations.len(), 2);
    let none = top(
        &shard,
        r#"{"query":{"term":{"tag":"nothing"}},"aggs":{"t":{"top_hits":{"explain":true}}}}"#,
    );
    assert!(none.explanations.is_empty());
}

#[test]
fn what_a_hit_carries_is_resolved_only_with_hits() {
    let shard = shard("th-fields");
    let t = top(
        &shard,
        r#"{"aggs":{"t":{"top_hits":{"docvalue_fields":["n"],"_source":["n"],"size":1}}}}"#,
    );
    assert_eq!(t.fields.len(), 1);
    assert!(!t.source.is_whole());
    let none = top(
        &shard,
        r#"{"query":{"term":{"tag":"nothing"}},"aggs":{"t":{"top_hits":{"docvalue_fields":["n"]}}}}"#,
    );
    assert!(none.fields.is_empty());
}

#[test]
fn a_top_hits_reserves_its_own_window_of_hits() {
    let shard = shard("th-bytes");
    let bytes = |aggs: &str| {
        hits_bytes(
            &source(&format!(r#"{{"size":0,"aggs":{aggs}}}"#)).expect("parses"),
            &shard,
        )
    };
    let none = bytes(r#"{"a":{"max":{"field":"n"}}}"#);
    let small = bytes(r#"{"t":{"top_hits":{"size":1}}}"#);
    let big = bytes(r#"{"t":{"top_hits":{"size":100}}}"#);
    let two = bytes(r#"{"t":{"top_hits":{"size":100}},"u":{"top_hits":{"size":100}}}"#);
    assert!(small > none, "{small} {none}");
    assert!(big > small, "{big} {small}");
    // A window past the limit counts no more than the limit: the search
    // refuses it.
    assert_eq!(bytes(r#"{"t":{"top_hits":{"size":5000}}}"#), big);
    // Each aggregation holds its own, and a keyword sort adds its terms.
    assert!(two > big + (big - none) / 2, "{two} {big}");
    let keyed = bytes(r#"{"t":{"top_hits":{"size":100,"sort":["tag"]}}}"#);
    assert!(keyed > big + 100 * 32_766, "{keyed} {big}");
}

#[test]
fn a_sorted_search_of_no_hits_still_counts_its_matches() {
    let shard = shard("th-sorted-zero");
    for body in [
        r#"{"size":0,"sort":[{"n":"asc"}]}"#,
        r#"{"size":0,"sort":[{"tag":"asc"}]}"#,
    ] {
        let source = source(body).expect("parses");
        let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
        assert_eq!((hits.total, hits.exact), (DOCS as u64, true), "{body}");
        assert!(hits.hits.is_empty() && hits.max_score.is_none(), "{body}");
    }
}

#[test]
fn the_hits_ahead_of_from_are_kept_for_the_coordinator_s_merge() {
    let shard = shard("th-before");
    let t = top(&shard, r#"{"aggs":{"t":{"top_hits":{"from":2,"size":2}}}}"#);
    assert_eq!(docs(&t), [2, 3]);
    let before: Vec<i32> = t.before.iter().map(|h| h.doc).collect();
    assert_eq!(before, [0, 1]);
    let t = top(&shard, r#"{"aggs":{"t":{"top_hits":{"size":2}}}}"#);
    assert!(t.before.is_empty());
    // Past the matches every hit is ahead of `from`.
    let t = top(
        &shard,
        r#"{"query":{"range":{"n":{"gte":10,"lt":20}}},"aggs":{"t":{"top_hits":{"from":50,"size":5}}}}"#,
    );
    assert!(t.hits.is_empty());
    assert_eq!(t.before.len(), 10);
}

#[test]
fn a_field_the_hits_cannot_carry_fails_only_when_a_hit_is_written() {
    let shard = shard("th-bad-fields");
    let bad = r#""docvalue_fields":["_id"]"#;
    // A hit to fetch: the refusal is the search's.
    let (status, reason) = failure(
        &shard,
        &format!(r#"{{"aggs":{{"t":{{"top_hits":{{{bad},"size":1}}}}}}}}"#),
    );
    assert_eq!(status, 400, "{reason}");
    // Only hits ahead of `from`: nothing is written here, so the shard
    // answers, with what the merge could pull left unresolved.
    let t = top(
        &shard,
        &format!(
            r#"{{"query":{{"range":{{"n":{{"gte":10,"lt":20}}}}}},"aggs":{{"t":{{"top_hits":{{{bad},"from":50}}}}}}}}"#
        ),
    );
    assert!(t.hits.is_empty() && t.before.len() == 10 && t.fields.is_empty());
}
