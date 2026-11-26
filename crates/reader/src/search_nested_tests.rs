//! Searches over an index of nested objects; compiled only under `#[cfg(test)]`.

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{NESTED_CHILDREN, Spec, blobs};

use crate::dsl::parse_chunks;
use crate::search::{Hits, ShardError, search};
use crate::{OpenShard, open_shard};

const DOCS: usize = 6;
/// Documents of one block: the children, then the parent.
const BLOCK: i32 = NESTED_CHILDREN as i32 + 1;

fn shard(spec: &Spec) -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", &spec.name, 0).expect("opens")
}

fn nested_shard() -> OpenShard<Emulator> {
    shard(&Spec::new("ns", DOCS).segments_of(4).with_nested())
}

fn query(shard: &OpenShard<Emulator>, query: &str) -> Result<Hits, ShardError> {
    let body = format!(r#"{{"size":20,"query":{query}}}"#);
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None))
}

/// The parent of document `i` is the last of its block.
fn parent(i: i32) -> i32 {
    i * BLOCK + BLOCK - 1
}

fn docs(h: &Hits) -> Vec<i32> {
    let mut docs: Vec<i32> = h.hits.iter().map(|h| h.doc).collect();
    docs.sort_unstable();
    docs
}

#[test]
fn a_search_of_an_index_with_nested_objects_sees_only_the_parents() {
    let shard = nested_shard();
    let all = query(&shard, r#"{"match_all":{}}"#).expect("searches");
    assert_eq!(all.total, DOCS as u64);
    assert_eq!(docs(&all), (0..DOCS as i32).map(parent).collect::<Vec<_>>());
    // A query that matches children by their own field finds no parent.
    let child = query(&shard, r#"{"term":{"c.w":"w2-1"}}"#).expect("searches");
    assert_eq!(child.total, 0);
}

#[test]
fn an_index_without_nested_objects_is_searched_as_before() {
    let shard = shard(&Spec::new("flat", DOCS));
    let all = query(&shard, r#"{"match_all":{}}"#).expect("searches");
    assert_eq!(all.total, DOCS as u64);
}

#[test]
fn a_nested_query_matches_the_parents_of_matching_children() {
    let shard = nested_shard();
    let hit = query(
        &shard,
        r#"{"nested":{"path":"c","query":{"term":{"c.w":"w2-1"}}}}"#,
    )
    .expect("searches");
    assert_eq!((hit.total, docs(&hit)), (1, vec![parent(2)]));
    let two = query(
        &shard,
        r#"{"nested":{"path":"c","query":{"terms":{"c.w":["w1-0","w4-1","nope"]}}}}"#,
    )
    .expect("searches");
    assert_eq!(docs(&two), vec![parent(1), parent(4)]);
    let none = query(
        &shard,
        r#"{"nested":{"path":"c","query":{"term":{"c.w":"nope"}}}}"#,
    )
    .expect("searches");
    assert_eq!(none.total, 0);
}

#[test]
fn a_nested_query_scores_its_parents_by_the_score_mode() {
    let shard = nested_shard();
    let score = |mode: &str| {
        let body = format!(
            r#"{{"nested":{{"path":"c","score_mode":"{mode}","query":{{"terms":{{"c.w":["w3-0","w3-1"]}}}}}}}}"#
        );
        let h = query(&shard, &body).expect("searches");
        assert_eq!(docs(&h), vec![parent(3)]);
        h.hits[0].score.expect("scored")
    };
    assert_eq!(score("none"), 0.0);
    assert_eq!(score("sum"), 2.0);
    assert_eq!(score("avg"), 1.0);
    assert_eq!(score("max"), 1.0);
    assert_eq!(score("min"), 1.0);
}

#[test]
fn a_nested_query_composes_with_the_queries_around_it() {
    let shard = nested_shard();
    let hit = query(
        &shard,
        r#"{"bool":{"must":{"nested":{"path":"c","query":{"term":{"c.w":"w2-1"}}}},"must_not":{"term":{"tag":"nope"}}}}"#,
    )
    .expect("searches");
    assert_eq!(docs(&hit), vec![parent(2)]);
    let excluded = query(
        &shard,
        r#"{"bool":{"must_not":{"nested":{"path":"c","query":{"term":{"c.w":"w2-1"}}}}}}"#,
    )
    .expect("searches");
    assert_eq!(excluded.total, DOCS as u64 - 1);
}

#[test]
fn an_unmapped_path_fails_unless_it_is_ignored() {
    let shard = nested_shard();
    let e = query(
        &shard,
        r#"{"nested":{"path":"d","query":{"match_all":{}}}}"#,
    )
    .expect_err("fails");
    assert!(
        format!("{:?}", e.failure).contains("[nested] failed to find nested object under path [d]"),
        "{:?}",
        e.failure
    );
    let ignored = query(
        &shard,
        r#"{"nested":{"path":"d","ignore_unmapped":true,"query":{"match_all":{}}}}"#,
    )
    .expect("searches");
    assert_eq!(ignored.total, 0);
    // An index without nested objects has no such path either.
    let flat = shard_flat();
    assert!(query(&flat, r#"{"nested":{"path":"c","query":{"match_all":{}}}}"#).is_err());
}

fn shard_flat() -> OpenShard<Emulator> {
    shard(&Spec::new("flat", DOCS))
}
