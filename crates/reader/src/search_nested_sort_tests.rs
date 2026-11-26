//! Sorts that order parents by their nested children; compiled only under
//! `#[cfg(test)]`.

// ARITH: test document ids over a handful of blocks.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{NESTED_CHILDREN, Spec, blobs};

use crate::dsl::parse_chunks;
use crate::search::{Hits, ShardError, SortValue, search};
use crate::{OpenShard, open_shard};

const DOCS: usize = 6;
/// Documents of one block: the children, then the parent.
const BLOCK: i32 = NESTED_CHILDREN as i32 + 1;

fn shard() -> OpenShard<Emulator> {
    let spec = Spec::new("nsort", DOCS).segments_of(4).with_nested();
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

fn sorted(shard: &OpenShard<Emulator>, rest: &str) -> Result<Hits, ShardError> {
    let body = format!(r#"{{"size":20,"query":{{"match_all":{{}}}}{rest}}}"#);
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None))
}

/// The parent of document `i` is the last of its block.
fn parent(i: i32) -> i32 {
    i * BLOCK + NESTED_CHILDREN as i32
}

fn order(h: &Hits) -> Vec<i32> {
    h.hits.iter().map(|h| h.doc).collect()
}

fn sort_of(order: &str, nested: &str) -> String {
    format!(r#","sort":[{{"c.w":{{"order":"{order}","nested":{nested}}}}}]"#)
}

#[test]
fn parents_come_by_their_greatest_child_descending_and_their_least_ascending() {
    let shard = shard();
    // Children are `w<i>-0` and `w<i>-1`: the least is `w<i>-0`, the
    // greatest `w<i>-1`, and both rise with `i`.
    let desc = sorted(&shard, &sort_of("desc", r#"{"path":"c"}"#)).expect("sorts");
    assert_eq!(desc.total, DOCS as u64);
    assert_eq!(
        order(&desc),
        (0..DOCS as i32).rev().map(parent).collect::<Vec<_>>()
    );
    let asc = sorted(&shard, &sort_of("asc", r#"{"path":"c"}"#)).expect("sorts");
    assert_eq!(
        order(&asc),
        (0..DOCS as i32).map(parent).collect::<Vec<_>>()
    );
    // A hit reports the child value that placed it.
    let reported = |h: &Hits| match &h.hits[0].sort[0] {
        SortValue::Term(Some(b)) => String::from_utf8(b.clone()).expect("utf8"),
        other => panic!("{other:?}"),
    };
    assert_eq!(reported(&desc), "w5-1");
    assert_eq!(reported(&asc), "w0-0");
}

#[test]
fn a_filter_chooses_the_children_that_count_and_a_parent_without_one_is_last() {
    let shard = shard();
    let filter = r#"{"path":"c","filter":{"terms":{"c.w":["w4-1","w1-1"]}}}"#;
    let asc = sorted(&shard, &sort_of("asc", filter)).expect("sorts");
    let rest = [0, 2, 3, 5];
    let expected =
        |first: [i32; 2]| -> Vec<i32> { first.into_iter().chain(rest).map(parent).collect() };
    assert_eq!(order(&asc), expected([1, 4]));
    let desc = sorted(&shard, &sort_of("desc", filter)).expect("sorts");
    assert_eq!(order(&desc), expected([4, 1]));
    // The older form says the same.
    let legacy = sorted(
        &shard,
        r#","sort":[{"c.w":{"order":"asc","nested_path":"c","nested_filter":{"terms":{"c.w":["w4-1","w1-1"]}}}}]"#,
    )
    .expect("sorts");
    assert_eq!(order(&legacy), order(&asc));
}

#[test]
fn a_sort_without_a_nested_object_sees_the_parents_own_values() {
    let shard = shard();
    // The parents hold no `c.w`: every one is missing and they keep their order.
    let plain = sorted(&shard, r#","sort":[{"c.w":"desc"}]"#).expect("sorts");
    assert_eq!(
        order(&plain),
        (0..DOCS as i32).map(parent).collect::<Vec<_>>()
    );
}

#[test]
fn a_path_that_is_not_a_nested_object_fails_in_words() {
    let shard = shard();
    let e = sorted(&shard, &sort_of("asc", r#"{"path":"d"}"#)).expect_err("fails");
    assert_eq!(e.status, 400);
    assert!(
        format!("{:?}", e.failure).contains("[nested] failed to find nested object under path [d]"),
        "{:?}",
        e.failure
    );
}

#[test]
fn search_after_with_a_nested_sort_is_refused() {
    let shard = shard();
    let e = sorted(
        &shard,
        &format!(
            r#"{},"search_after":["w1-1"]"#,
            sort_of("asc", r#"{"path":"c"}"#)
        ),
    )
    .expect_err("fails");
    assert!(
        format!("{:?}", e.failure).contains("is not supported by this node yet"),
        "{:?}",
        e.failure
    );
}
