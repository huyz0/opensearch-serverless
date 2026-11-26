//! `nested` and `reverse_nested` as sub-aggregations of a `terms`, on an
//! index of blocks (six documents of two children each, one `tag` and one
//! `n` each).
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use super::filters_render::write_filters;
use super::keyword_collect::Parent;
use super::keyword_subs;
use super::keyword_terms::write_keyword_terms;
use super::terms_render::write_terms;
use crate::aggs::partial::{Format, Partial, Value, reduce};
use crate::dsl::parse_chunks;
use crate::search::{ShardError, hits_bytes, search};
use crate::{OpenShard, open_shard};

const DOCS: usize = 6;

fn shard() -> OpenShard<Emulator> {
    let spec = Spec::new("nut", DOCS).segments_of(4).with_nested();
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
    open_shard(Arc::new(store), cache, "", "nut", 0).expect("opens")
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> Result<Vec<Partial>, ShardError> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None)).map(|h| h.aggregations)
}

fn written(value: &Value) -> String {
    let mut out = Vec::new();
    match value {
        Value::KeywordTerms(r) => write_keyword_terms(&mut out, r).expect("writes"),
        Value::Filters(r) => write_filters(&mut out, r).expect("writes"),
        Value::Terms(r) => write_terms(&mut out, Format::Raw, r).expect("writes"),
        other => panic!("not a terms or nested: {other:?}"),
    }
    String::from_utf8(out).expect("utf-8")
}

fn body_of(shard: &OpenShard<Emulator>, body: &str) -> String {
    written(&run(shard, body).expect("searches").remove(0).value)
}

// AC-9
#[test]
fn a_nested_under_a_keyword_terms_counts_each_buckets_own_children() {
    let got = body_of(
        &shard(),
        r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag","size":10,"order":{"_key":"asc"}},
            "aggs":{"c":{"nested":{"path":"c"},"aggs":{"w":{"terms":{"field":"c.w","size":10}}}}}}}}"#,
    );
    for i in 0..DOCS {
        // Each tag holds one document with two children.
        assert!(
            got.contains(&format!(
                r#""key":"t{i}","doc_count":1,"c":{{"doc_count":2"#
            )),
            "{got}"
        );
        for j in 0..2 {
            assert!(
                got.contains(&format!(r#""key":"w{i}-{j}","doc_count":1"#)),
                "{got}"
            );
        }
    }
}

// AC-9
#[test]
fn a_nested_under_a_numeric_terms_counts_each_buckets_own_children() {
    let got = body_of(
        &shard(),
        r#"{"size":0,"query":{"range":{"n":{"lt":2}}},"aggs":{"t":{"terms":{"field":"n","size":10,"order":{"_key":"asc"}},
            "aggs":{"c":{"nested":{"path":"c"}}}}}}"#,
    );
    assert!(
        got.contains(r#""key":0,"doc_count":1,"c":{"doc_count":2}"#),
        "{got}"
    );
    assert!(
        got.contains(r#""key":1,"doc_count":1,"c":{"doc_count":2}"#),
        "{got}"
    );
}

// AC-9
#[test]
fn the_results_of_shards_add_under_a_terms() {
    let shard = shard();
    let body = r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag","size":10},
        "aggs":{"c":{"nested":{"path":"c"}}}}}}"#;
    let one = run(&shard, body).expect("searches").remove(0).value;
    let both = reduce(&[one.clone(), one]).expect("reduces");
    let got = written(&both);
    assert!(
        got.contains(r#""key":"t0","doc_count":2,"c":{"doc_count":4}"#),
        "{got}"
    );
}

// AC-10
#[test]
fn a_reverse_nested_under_a_terms_counts_the_parents_of_each_term() {
    let shard = shard();
    let got = body_of(
        &shard,
        r#"{"size":0,"aggs":{"c":{"nested":{"path":"c"},"aggs":{"w":{"terms":{"field":"c.w","size":20,
            "order":{"_key":"asc"}},"aggs":{"p":{"reverse_nested":{},"aggs":{"s":{"sum":{"field":"n"}}}}}}}}}}"#,
    );
    // Child w<i>-<j> sits under document i, whose n is i.
    for i in 0..DOCS {
        for j in 0..2 {
            assert!(
                got.contains(&format!(
                    r#""key":"w{i}-{j}","doc_count":1,"p":{{"doc_count":1,"s":{{"value":{i}"#
                )),
                "{got}"
            );
        }
    }
}

// AC-10
#[test]
fn a_reverse_nested_under_a_terms_outside_a_nested_is_refused_as_the_java_refuses() {
    let err = run(
        &shard(),
        r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"p":{"reverse_nested":{}}}}}}"#,
    )
    .expect_err("refused");
    assert_eq!(err.status, 400);
    assert_eq!(err.failure.kind, "illegal_argument_exception");
    assert!(
        err.failure.reason.contains(
            "Reverse nested aggregation [p] can only be used inside a [nested] aggregation"
        ),
        "{}",
        err.failure.reason
    );
}

// AC-11
#[test]
fn a_nested_under_a_keyword_terms_is_reserved_with_what_it_holds() {
    let shard = shard();
    let bytes = |body: &str| hits_bytes(&parse_chunks([body.as_bytes()]).expect("parses"), &shard);
    let bare = bytes(
        r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"c":{"nested":{"path":"c"}}}}}}"#,
    );
    let holding = bytes(
        r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"c":{"nested":{"path":"c"},
            "aggs":{"w":{"terms":{"field":"c.w"}}}}}}}}"#,
    );
    assert!(holding > bare, "{holding} vs {bare}");
}

// AC-11
#[test]
fn a_nested_bucket_under_a_keyword_terms_adds_to_the_terms_reservation() {
    let shard = shard();
    let held = |body: &str| {
        let source = parse_chunks([body.as_bytes()]).expect("parses");
        keyword_subs::reserved(&shard, &source.aggs[0], Parent::TOP)
    };
    let bare = held(r#"{"aggs":{"t":{"terms":{"field":"tag"}}}}"#);
    let with =
        held(r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"c":{"nested":{"path":"c"}}}}}}"#);
    assert!(with > bare, "{with} vs {bare}");
}
