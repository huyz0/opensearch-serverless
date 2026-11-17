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

use super::*;
use crate::dsl::parse_chunks;
use crate::response::write_response;
use crate::search::search;
use crate::{OpenShard, open_shard};

fn shard() -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    let spec = Spec::new("ex", 20).segments_of(10).with_keywords(2);
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "ex", 0).expect("opens")
}

fn respond(shard: &OpenShard<Emulator>, body: &str) -> (usize, String) {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let hits = search(shard, "ex", &source, &Analyzer::standard(None)).expect("searches");
    let mut out = Vec::new();
    write_response(&mut out, shard, "ex", "node-1", &hits, 0, &mut |_| Ok(())).expect("writes");
    (
        hits.explanations.len(),
        String::from_utf8(out).expect("utf-8"),
    )
}

#[test]
fn an_explanation_is_written_as_value_description_and_details() {
    let e = Explanation::match_(0.5, "a \"quoted\" part, result of:").with_details(vec![
        Explanation::match_long(2, "n"),
        Explanation::match_(1.5, "second"),
    ]);
    let mut out = Vec::new();
    write_explanation(&mut out, &e).expect("writes");
    assert_eq!(
        String::from_utf8(out).expect("utf-8"),
        r#","_explanation":{"value":0.5,"description":"a \"quoted\" part, result of:","details":[{"value":2,"description":"n","details":[]},{"value":1.5,"description":"second","details":[]}]}"#
    );
}

#[test]
fn each_hit_carries_its_shard_node_and_explanation_after_its_sort() {
    let shard = shard();
    let (n, body) = respond(
        &shard,
        r#"{"explain":true,"size":2,"sort":[{"n":"asc"}],"query":{"match_all":{}}}"#,
    );
    assert_eq!(n, 2);
    assert_eq!(
        body.matches(r#"{"_shard":"[ex][0]","_node":"node-1","_index":"ex","_id""#)
            .count(),
        2,
        "{body}"
    );
    assert_eq!(
        body.matches(
            r#","sort":[0],"_explanation":{"value":1.0,"description":"*:*","details":[]}}"#
        )
        .count(),
        1,
        "{body}"
    );
    // Without `explain`, a hit carries neither.
    let (n, plain) = respond(&shard, r#"{"size":2,"query":{"match_all":{}}}"#);
    assert_eq!(n, 0);
    assert!(
        !plain.contains("\"_shard\"") && !plain.contains("_explanation"),
        "{plain}"
    );
    // A search with no hit has nothing to explain.
    let (n, none) = respond(
        &shard,
        r#"{"explain":true,"query":{"term":{"tag":"no-such-tag"}}}"#,
    );
    assert_eq!(n, 0);
    assert!(!none.contains("_explanation"), "{none}");
}

fn refused(shard: &OpenShard<Emulator>, query: &str) -> String {
    let body = format!(r#"{{"explain":true,"query":{query}}}"#);
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let e = search(shard, "ex", &source, &Analyzer::standard(None)).expect_err("refused");
    assert_eq!(e.status, 400);
    e.failure.reason
}

#[test]
fn a_query_the_engine_does_not_explain_as_opensearch_does_is_refused() {
    let shard = shard();
    for (query, what) in [
        (
            r#"{"range":{"n":{"gte":1}}}"#,
            "a range or other points query",
        ),
        (r#"{"term":{"n":1}}"#, "a range or other points query"),
        (r#"{"exists":{"field":"n"}}"#, "an exists query"),
        (r#"{"ids":{"values":["doc.1"]}}"#, "an ids query"),
        (
            r#"{"constant_score":{"filter":{"term":{"tag":"t1"}},"boost":2}}"#,
            "a constant_score query with a boost",
        ),
        // A boosted constant score, and a boosted bool holding an exists.
        (
            r#"{"exists":{"field":"n","boost":2}}"#,
            "a constant_score query with a boost",
        ),
        (
            r#"{"bool":{"must":[{"exists":{"field":"n"}}],"boost":2}}"#,
            "an exists query",
        ),
        // Found under a bool, a dis_max and a constant_score.
        (
            r#"{"bool":{"must":[{"dis_max":{"queries":[{"constant_score":{"filter":{"exists":{"field":"n"}}}}]}}]}}"#,
            "an exists query",
        ),
        (
            r#"{"bool":{"filter":[{"bool":{"should":[{"range":{"n":{"lt":3}}}]}}]}}"#,
            "a range or other points query",
        ),
    ] {
        assert_eq!(
            refused(&shard, query),
            format!("explain of {what} is not supported by this node yet"),
            "{query}"
        );
    }
}

#[test]
fn explanations_name_the_queries_as_opensearch_names_them() {
    let shard = shard();
    let explained = |query: &str| {
        respond(
            &shard,
            &format!(r#"{{"explain":true,"size":1,"query":{query}}}"#),
        )
        .1
    };
    // A tie breaker of 1 is a sum, as Lucene's rewrite makes it.
    let sum = explained(
        r#"{"dis_max":{"queries":[{"term":{"tag":"t1"}},{"term":{"k0":"v1"}}],"tie_breaker":1}}"#,
    );
    assert!(sum.contains(r#""description":"sum of:""#), "{sum}");
    let max = explained(
        r#"{"dis_max":{"queries":[{"term":{"tag":"t1"}},{"term":{"k0":"v1"}}],"tie_breaker":0.3}}"#,
    );
    assert!(
        max.contains(r#""description":"max plus 0.3 times others of:""#),
        "{max}"
    );
    // A constant score over a keyword term names the term once.
    let constant = explained(r#"{"constant_score":{"filter":{"term":{"tag":"t1"}}}}"#);
    assert!(
        constant.contains(r#""description":"ConstantScore(tag:t1)""#),
        "{constant}"
    );
    assert!(
        !constant.contains("ConstantScore(ConstantScore"),
        "{constant}"
    );
    // A filter names the query it filters by.
    let filter =
        explained(r#"{"bool":{"must":[{"term":{"k0":"v1"}}],"filter":[{"term":{"tag":"t1"}}]}}"#);
    assert!(
        filter.contains(r#""description":"match on required clause, product of:""#),
        "{filter}"
    );
    assert!(filter.contains(r#""description":"tag:t1""#), "{filter}");
    // A boosted text query, and terms over a field other than `_id`, are
    // explained rather than refused.
    explained(r#"{"match":{"pad":{"query":"a","boost":2}}}"#);
    explained(r#"{"bool":{"must":[{"match":{"pad":{"query":"a","boost":3}}}]}}"#);
    let terms = explained(r#"{"terms":{"tag":["t1","t2"]}}"#);
    assert!(terms.contains("_explanation"), "{terms}");
    // The per-field similarity names itself.
    let text = explained(r#"{"match":{"pad":"a"}}"#);
    assert!(!text.contains("[BM25Similarity]"), "{text}");
}
