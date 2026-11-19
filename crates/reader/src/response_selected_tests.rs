//! Selected hits of a `top_hits` result written as the response writes them:
//! docs/specs/coordinator-top-hits-render, AC-1 to AC-4.
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

use super::{render_selected_hits, render_selected_result_hits, write_response};
use crate::aggs::metric::{TopHitsResult, Value};
use crate::dsl::parse_chunks;
use crate::search::{Hits, search};
use crate::{OpenShard, open_shard};

fn spec() -> Spec {
    Spec::new("sel", 20).segments_of(10).padded(40)
}

fn shard() -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", &spec()) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "sel", 0).expect("opens")
}

fn searched(shard: &OpenShard<Emulator>, top_hits: &str) -> Hits {
    let body = format!(r#"{{"size":0,"aggs":{{"t":{{"top_hits":{top_hits}}}}}}}"#);
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None)).expect("searches")
}

fn result(hits: Hits) -> TopHitsResult {
    match hits.aggregations.into_iter().next().map(|p| p.value) {
        Some(Value::TopHits(t)) => *t,
        other => panic!("not a top_hits: {other:?}"),
    }
}

fn top(shard: &OpenShard<Emulator>, top_hits: &str) -> TopHitsResult {
    result(searched(shard, top_hits))
}

fn render(shard: &OpenShard<Emulator>, result: &TopHitsResult, positions: &[usize]) -> Vec<String> {
    render_selected_hits(shard, "idx", "node", result, positions, &mut |_| Ok(()))
        .expect("renders")
        .into_iter()
        .map(|h| String::from_utf8(h).expect("utf-8"))
        .collect()
}

/// The `hits` list of the response's `top_hits`, as the response writes it.
fn response_hits(shard: &OpenShard<Emulator>, top_hits: &str) -> String {
    let hits = searched(shard, top_hits);
    let mut out = Vec::new();
    write_response(&mut out, shard, "idx", "node", &hits, 0, &mut |_| Ok(())).expect("writes");
    let body = String::from_utf8(out).expect("utf-8");
    let at = body.find(r#""aggregations""#).expect("aggregations");
    let from = body[at..].find(r#""hits":["#).expect("hits") + at + 8;
    let end = body[from..].rfind("]}").expect("end") + from;
    body[from..end].to_string()
}

#[test]
fn the_hits_ahead_of_from_are_explained_only_when_asked() {
    let shard = shard();
    let t = top(&shard, r#"{"from":2,"size":2,"explain":true}"#);
    assert_eq!((t.before.len(), t.before_explanations.len()), (2, 2));
    assert_eq!(t.explanations.len(), 2);
    let t = top(&shard, r#"{"from":2,"size":2}"#);
    assert!(t.before_explanations.is_empty() && t.explanations.is_empty());
}

#[test]
fn selected_hits_are_written_as_the_response_writes_them() {
    let shard = shard();
    let t = top(&shard, r#"{"from":3,"size":3}"#);
    let all = response_hits(&shard, r#"{"from":0,"size":6}"#);
    let picked = render(&shard, &t, &[0, 2, 4, 5]);
    assert_eq!(picked.len(), 4);
    let whole = render(&shard, &top(&shard, r#"{"size":6}"#), &[0, 1, 2, 3, 4, 5]);
    assert_eq!(whole.join(","), all);
    for (got, at) in picked.iter().zip([0, 2, 4, 5]) {
        assert_eq!(got, &whole[at]);
    }
}

#[test]
fn a_selection_follows_the_order_asked_and_may_repeat() {
    let shard = shard();
    let t = top(&shard, r#"{"from":1,"size":2}"#);
    let one = render(&shard, &t, &[0, 1, 2]);
    let back = render(&shard, &t, &[2, 0, 0]);
    assert_eq!(back, [one[2].clone(), one[0].clone(), one[0].clone()]);
}

#[test]
fn explain_sort_fields_and_source_filtering_are_written() {
    let shard = shard();
    let options = r#"{"sort":[{"n":"desc"}],"_source":["n"],"docvalue_fields":["n"],
        "explain":true,"size":5}"#;
    let full = top(&shard, options);
    let ahead = top(
        &shard,
        &options.replace(r#""size":5"#, r#""size":3,"from":2"#),
    );
    let all = response_hits(&shard, options);
    assert!(
        all.contains(r#""_explanation""#) && all.contains(r#""sort":["#),
        "{all}"
    );
    assert!(all.contains(r#""fields":{"n""#), "{all}");
    let whole = render(&shard, &full, &[0, 1, 2, 3, 4]);
    assert_eq!(whole.join(","), all);
    // Positions 0 and 1 are `before` in `ahead`; 2 and 4 are in `hits`.
    let picked = render(&shard, &ahead, &[1, 2, 4]);
    assert_eq!(
        picked,
        [whole[1].clone(), whole[2].clone(), whole[4].clone()]
    );
}

#[test]
fn no_position_or_one_beyond_the_result_is_refused() {
    let shard = shard();
    let t = top(&shard, r#"{"from":1,"size":2}"#);
    let run = |positions: &[usize]| {
        render_selected_hits(&shard, "idx", "node", &t, positions, &mut |_| Ok(()))
    };
    assert!(run(&[]).is_err());
    assert!(run(&[0, 3]).is_err());
    assert!(run(&[3]).is_err());
    assert_eq!(run(&[2]).expect("last").len(), 1);
}

fn searched_top(shard: &OpenShard<Emulator>, body: &str) -> Hits {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None)).expect("searches")
}

/// The `hits` list of the response's top-level hits, as it is written.
fn response_top(shard: &OpenShard<Emulator>, hits: &Hits) -> String {
    let mut out = Vec::new();
    write_response(&mut out, shard, "idx", "node", hits, 0, &mut |_| Ok(())).expect("writes");
    let body = String::from_utf8(out).expect("utf-8");
    let from = body.find(r#""hits":["#).expect("hits") + 8;
    let end = body[from..].rfind("]}").expect("end") + from;
    body[from..end].to_string()
}

fn render_top(shard: &OpenShard<Emulator>, hits: &Hits, positions: &[usize]) -> Vec<String> {
    render_selected_result_hits(shard, "idx", "node", hits, positions, &mut |_| Ok(()))
        .expect("renders")
        .into_iter()
        .map(|h| String::from_utf8(h).expect("utf-8"))
        .collect()
}

#[test]
fn selected_result_hits_are_written_as_the_response_writes_them() {
    let shard = shard();
    let hits = searched_top(&shard, r#"{"size":6}"#);
    let all = response_top(&shard, &hits);
    let whole = render_top(&shard, &hits, &[0, 1, 2, 3, 4, 5]);
    assert_eq!(whole.join(","), all);
    let picked = render_top(&shard, &hits, &[5, 0, 0, 3]);
    assert_eq!(
        picked,
        [
            whole[5].clone(),
            whole[0].clone(),
            whole[0].clone(),
            whole[3].clone()
        ]
    );
}

#[test]
fn selected_result_hits_carry_explain_sort_fields_and_source_filtering() {
    let shard = shard();
    for (options, marks) in [
        (
            r#"{"_source":["n"],"docvalue_fields":["n"],"explain":true,"size":4}"#,
            [r#""_explanation""#, r#""fields":{"n""#],
        ),
        (
            r#"{"sort":[{"n":"desc"}],"_source":["n"],"docvalue_fields":["n"],"size":4}"#,
            [r#""sort":["#, r#""fields":{"n""#],
        ),
    ] {
        let hits = searched_top(&shard, options);
        let all = response_top(&shard, &hits);
        for mark in marks {
            assert!(all.contains(mark), "{mark} in {all}");
        }
        let whole = render_top(&shard, &hits, &[0, 1, 2, 3]);
        assert_eq!(whole.join(","), all);
        assert_eq!(
            render_top(&shard, &hits, &[3, 1]),
            [whole[3].clone(), whole[1].clone()]
        );
    }
}

#[test]
fn no_position_or_one_beyond_the_result_hits_is_refused() {
    let shard = shard();
    let hits = searched_top(&shard, r#"{"size":3}"#);
    let run = |positions: &[usize]| {
        render_selected_result_hits(&shard, "idx", "node", &hits, positions, &mut |_| Ok(()))
    };
    assert!(run(&[]).is_err());
    assert!(run(&[0, 3]).is_err());
    assert_eq!(run(&[2]).expect("last").len(), 1);
}
