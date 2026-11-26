//! `inner_hits` of a nested query, searched and written from a real index of
//! nested objects; compiled only under `#[cfg(test)]`.

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use crate::dsl::parse_chunks;
use crate::response::write_response;
use crate::search::{ShardError, search};
use crate::{OpenShard, open_shard};

fn shard() -> OpenShard<Emulator> {
    let spec = Spec::new("ih", 6).segments_of(4).with_nested();
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", &spec.name, 0).expect("opens")
}

/// The response for a nested query on `c` over `inner`, with `extra` inside
/// the nested object after its query.
fn respond(inner: &str, extra: &str) -> Result<String, ShardError> {
    let nested = format!(r#"{{"nested":{{"path":"c","query":{inner}{extra}}}}}"#);
    respond_query(&nested)
}

/// The response for `query`.
fn respond_query(query: &str) -> Result<String, ShardError> {
    let shard = shard();
    let body = format!(r#"{{"size":20,"query":{query}}}"#);
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let hits = search(&shard, "idx", &source, &Analyzer::standard(None))?;
    let mut out = Vec::new();
    write_response(&mut out, &shard, "idx", "node", &hits, 0, &mut |_| Ok(())).expect("writes");
    Ok(String::from_utf8(out).expect("utf-8"))
}

fn reason(inner: &str, extra: &str) -> String {
    respond(inner, extra).expect_err("fails").failure.reason
}

const BOTH: &str = r#"{"prefix":{"c.w":"w2"}}"#;

#[test]
fn a_parent_carries_the_children_that_matched_under_the_path() {
    let out = respond(r#"{"term":{"c.w":"w2-1"}}"#, r#","inner_hits":{}"#).expect("searches");
    assert!(
        out.contains(r#""inner_hits":{"c":{"hits":{"total":{"value":1,"relation":"eq"},"#),
        "{out}"
    );
    assert!(
        out.contains(r#""_nested":{"field":"c","offset":1}"#),
        "{out}"
    );
    assert!(out.contains(r#""_source":{"w":"w2-1"}"#), "{out}");
}

#[test]
fn from_and_size_page_the_children_and_the_total_counts_them_all() {
    let all = respond(BOTH, r#","inner_hits":{"size":5}"#).expect("searches");
    assert!(
        all.contains(r#""total":{"value":2,"relation":"eq"}"#),
        "{all}"
    );
    assert!(
        all.contains(r#""offset":0"#) && all.contains(r#""offset":1"#),
        "{all}"
    );
    let page = respond(BOTH, r#","inner_hits":{"from":1,"size":1}"#).expect("searches");
    assert!(
        page.contains(r#""total":{"value":2,"relation":"eq"}"#),
        "{page}"
    );
    assert!(
        page.contains(r#""offset":1"#) && !page.contains(r#""offset":0"#),
        "{page}"
    );
}

#[test]
fn the_children_follow_the_inner_sort_and_report_its_values() {
    let out =
        respond(BOTH, r#","inner_hits":{"size":1,"sort":[{"c.w":"desc"}]}"#).expect("searches");
    assert!(out.contains(r#""_source":{"w":"w2-1"}"#), "{out}");
    assert!(
        out.contains(r#""offset":1"#) && out.contains(r#""sort":["w2-1"]"#),
        "{out}"
    );
}

#[test]
fn the_name_replaces_the_path_and_the_source_can_be_dropped() {
    let out = respond(BOTH, r#","inner_hits":{"name":"kids","_source":false}"#).expect("searches");
    assert!(out.contains(r#""inner_hits":{"kids":{"hits""#), "{out}");
    assert!(
        !out.contains(r#""_nested":{"field":"c","offset":0},"_score":1.0,"_source""#),
        "{out}"
    );
    assert!(out.contains(r#""_nested""#), "{out}");
}

#[test]
fn a_nested_query_without_inner_hits_writes_none() {
    let out = respond(BOTH, "").expect("searches");
    assert!(!out.contains("inner_hits"), "{out}");
}

#[test]
fn a_window_over_the_limit_and_a_repeated_name_fail_in_words() {
    assert!(
        reason(BOTH, r#","inner_hits":{"from":50,"size":51}"#).contains(
            "Inner result window is too large, the inner hit definition's [c]'s from + size must \
             be less than or equal to: [100] but was [101]."
        )
    );
    let twice = r#"{"bool":{"must":[
        {"nested":{"path":"c","query":{"match_all":{}},"inner_hits":{}}},
        {"nested":{"path":"c","query":{"match_all":{}},"inner_hits":{}}}]}}"#;
    let reason = respond_query(twice).expect_err("fails").failure.reason;
    assert!(
        reason.contains("already contains an entry for key [c]"),
        "{reason}"
    );
}
