//! `nested` and `reverse_nested` run on an index of blocks (six documents of
//! two children each, in segments of four).
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use super::filters_render::write_filters;
use crate::aggs::partial::{Partial, Value, reduce};
use crate::dsl::parse_chunks;
use crate::search::{ShardError, search};
use crate::{OpenShard, open_shard};

const DOCS: usize = 6;
/// Children of every document.
const CHILDREN: u64 = 2;

fn shard() -> OpenShard<Emulator> {
    let spec = Spec::new("na", DOCS).segments_of(4).with_nested();
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
    open_shard(Arc::new(store), cache, "", "na", 0).expect("opens")
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Vec<Partial>, ShardError> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
}

fn written(value: &Value) -> String {
    let Value::Filters(r) = value else {
        panic!("not a nested: {value:?}");
    };
    let mut out = Vec::new();
    write_filters(&mut out, r).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

fn body_of(shard: &OpenShard<Emulator>, body: &str) -> String {
    written(&run(shard, body).expect("searches").remove(0).value)
}

// AC-3
#[test]
fn a_nested_counts_the_children_of_the_documents_collected() {
    let shard = shard();
    let all = body_of(&shard, r#"{"size":0,"aggs":{"n":{"nested":{"path":"c"}}}}"#);
    assert_eq!(all, format!(r#""doc_count":{}"#, DOCS as u64 * CHILDREN));
    // Documents 0 to 2 are collected: their six children.
    let some = body_of(
        &shard,
        r#"{"size":0,"query":{"range":{"n":{"lt":3}}},"aggs":{"n":{"nested":{"path":"c"}}}}"#,
    );
    assert_eq!(some, r#""doc_count":6"#);
}

// AC-3
#[test]
fn a_nested_runs_its_sub_aggregations_over_the_children() {
    let shard = shard();
    let got = body_of(
        &shard,
        r#"{"size":0,"query":{"range":{"n":{"lt":2}}},"aggs":{"n":{"nested":{"path":"c"},
            "aggs":{"w":{"terms":{"field":"c.w","size":10}}}}}}"#,
    );
    assert!(got.starts_with(r#""doc_count":4"#), "{got}");
    for word in ["w0-0", "w0-1", "w1-0", "w1-1"] {
        assert!(
            got.contains(&format!(r#""key":"{word}","doc_count":1"#)),
            "{got}"
        );
    }
    assert!(!got.contains("w2-0"), "{got}");
}

// AC-4
#[test]
fn a_reverse_nested_counts_each_parent_once_and_reads_its_fields() {
    let shard = shard();
    let got = body_of(
        &shard,
        r#"{"size":0,"aggs":{"n":{"nested":{"path":"c"},"aggs":{"r":{"reverse_nested":{},
            "aggs":{"s":{"sum":{"field":"n"}}}}}}}}"#,
    );
    // The nested bucket holds twelve children; the reverse bucket six parents
    // whose `n` are 0 to 5.
    assert!(got.starts_with(r#""doc_count":12"#), "{got}");
    assert!(
        got.contains(r#""r":{"doc_count":6,"s":{"value":15"#),
        "{got}"
    );
}

// AC-4
#[test]
fn a_reverse_nested_with_a_path_hands_the_nested_documents_of_that_path() {
    let shard = shard();
    let got = body_of(
        &shard,
        r#"{"size":0,"aggs":{"n":{"nested":{"path":"c"},"aggs":{"r":{"reverse_nested":{"path":"c"}}}}}}"#,
    );
    assert!(got.contains(r#""r":{"doc_count":12}"#), "{got}");
}

// AC-5
#[test]
fn an_unmapped_path_or_a_leaf_field_counts_nothing_and_its_sub_aggregations_are_empty() {
    let shard = shard();
    for path in ["nope", "n", "tag"] {
        let got = body_of(
            &shard,
            &format!(
                r#"{{"size":0,"aggs":{{"n":{{"nested":{{"path":"{path}"}},"aggs":{{"w":{{"terms":{{"field":"{path}.w"}}}}}}}}}}}}"#
            ),
        );
        assert!(got.starts_with(r#""doc_count":0"#), "{got}");
        assert!(
            got.contains(r#""w":{"#) && got.contains(r#""buckets":[]"#),
            "{got}"
        );
    }
}

// AC-5
#[test]
fn a_reverse_nested_of_an_unmapped_path_counts_nothing_and_a_stray_one_fails() {
    let shard = shard();
    let got = body_of(
        &shard,
        r#"{"size":0,"aggs":{"n":{"nested":{"path":"c"},"aggs":{"r":{"reverse_nested":{"path":"nope"}}}}}}"#,
    );
    assert!(got.contains(r#""r":{"doc_count":0}"#), "{got}");
    let e = run(&shard, r#"{"aggs":{"r":{"reverse_nested":{}}}}"#).expect_err("refused");
    assert!(e.failure.reason.ends_with(
        "Reverse nested aggregation [r] can only be used inside a [nested] aggregation"
    ));
}

// AC-6
#[test]
fn the_results_of_shards_add() {
    let shard = shard();
    let one = run(&shard, r#"{"size":0,"aggs":{"n":{"nested":{"path":"c"}}}}"#)
        .expect("searches")
        .remove(0)
        .value;
    let both = reduce(&[one.clone(), one]).expect("reduces");
    assert_eq!(written(&both), r#""doc_count":24"#);
}

// AC-7
#[test]
fn a_scored_sub_aggregation_is_refused_by_name_beneath_a_nested_at_any_depth() {
    for subs in [
        r#"{"t":{"top_hits":{}}}"#,
        r#"{"f":{"filter":{"match_all":{}},"aggs":{"t":{"top_hits":{}}}}}"#,
    ] {
        let body = format!(r#"{{"aggs":{{"n":{{"nested":{{"path":"c"}},"aggs":{subs}}}}}}}"#);
        let reason = parse_chunks([body.as_bytes()])
            .expect_err("refused")
            .into_failure()
            .reason
            .clone();
        assert!(
            reason.contains("[top_hits]") && reason.contains("[nested]"),
            "{reason}"
        );
    }
}
