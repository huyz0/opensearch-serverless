//! What a search reserves for its nested joins' parent bit sets; compiled
//! only under `#[cfg(test)]`.

use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use crate::dsl::parse_chunks;
use crate::search::hits_bytes;
use crate::search_nested_bytes::{bytes, list_bytes};
use crate::{OpenShard, open_shard};

/// Six blocks of three documents in segments of four documents: two
/// segments of 12 and 6 Lucene documents.
fn shard() -> OpenShard<Emulator> {
    let spec = Spec::new("nb", 6).segments_of(4).with_nested();
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
    open_shard(Arc::new(store), cache, "", "nb", 0).expect("opens")
}

/// One bit set: 18 documents are three bytes, and each of two segments adds
/// one word of slack.
const ONE: usize = 3 + 8 * 2;

const NESTED: &str = r#"{"nested":{"path":"c","query":{"match_all":{}}}}"#;

/// What the nested joins of `body` reserve.
fn of(shard: &OpenShard<Emulator>, body: &str) -> usize {
    bytes(&parse_chunks([body.as_bytes()]).expect("parses"), shard)
}

fn query(q: &str) -> String {
    format!(r#"{{"query":{q}}}"#)
}

// AC-1
#[test]
fn a_search_with_no_nested_join_reserves_nothing_for_one() {
    let shard = shard();
    for body in [
        r#"{"size":3}"#,
        r#"{"query":{"bool":{"must":[{"match_all":{}}]}},"aggs":{"f":{"filter":{"match_all":{}}}}}"#,
        r#"{"sort":[{"n":"asc"}],"aggs":{"t":{"terms":{"field":"n"}}}}"#,
    ] {
        assert_eq!(of(&shard, body), 0, "{body}");
    }
}

// AC-2, AC-4
#[test]
fn each_nested_query_node_adds_one_bit_set() {
    let shard = shard();
    assert_eq!(of(&shard, &query(NESTED)), ONE);
    assert_eq!(of(&shard, &format!(r#"{{"post_filter":{NESTED}}}"#)), ONE);
    let three =
        format!(r#"{{"bool":{{"must":[{NESTED}],"should":[{NESTED}],"filter":[{NESTED}]}}}}"#);
    assert_eq!(of(&shard, &query(&three)), 3 * ONE);
    let wrapped = format!(
        r#"{{"dis_max":{{"queries":[{{"constant_score":{{"filter":{NESTED}}}}},{{"function_score":{{"query":{NESTED}}}}}]}}}}"#
    );
    assert_eq!(of(&shard, &query(&wrapped)), 2 * ONE);
    let others = format!(
        r#"{{"bool":{{"must_not":[{NESTED}],"must":[{{"function_score":{{"functions":[{{"filter":{NESTED},"weight":2}}]}}}},{{"knn":{{"v":{{"vector":[1,2,3],"k":1,"filter":{NESTED}}}}}}}]}}}}"#
    );
    assert_eq!(of(&shard, &query(&others)), 3 * ONE);
    let inside =
        r#"{"nested":{"path":"c","query":{"nested":{"path":"c.d","query":{"match_all":{}}}}}}"#;
    assert_eq!(of(&shard, &query(inside)), 2 * ONE);
    let in_agg = format!(
        r#"{{"aggs":{{"f":{{"filter":{NESTED}}},"g":{{"filters":{{"filters":{{"a":{NESTED},"b":{{"match_all":{{}}}}}}}}}}}}}}"#
    );
    assert_eq!(of(&shard, &in_agg), 2 * ONE);
}

// AC-2
#[test]
fn inner_hits_add_a_bit_set_of_their_own() {
    let shard = shard();
    let with = r#"{"nested":{"path":"c","query":{"match_all":{}},"inner_hits":{}}}"#;
    assert_eq!(of(&shard, &query(with)), 2 * ONE);
}

// AC-3
#[test]
fn nested_aggregations_and_nested_sorts_each_add_one() {
    let shard = shard();
    assert_eq!(of(&shard, r#"{"aggs":{"a":{"nested":{"path":"c"}}}}"#), ONE);
    let under = r#"{"aggs":{"a":{"nested":{"path":"c"},"aggs":{"r":{"reverse_nested":{}}}}}}"#;
    assert_eq!(of(&shard, under), 2 * ONE);
    let sorted = r#"{"sort":[{"c.w":{"order":"asc","nested":{"path":"c"}}}]}"#;
    assert_eq!(of(&shard, sorted), ONE);
    let filtered =
        format!(r#"{{"sort":[{{"c.w":{{"nested":{{"path":"c","filter":{NESTED}}}}}}}]}}"#);
    assert_eq!(of(&shard, &filtered), 2 * ONE);
}

// AC-4
#[test]
fn a_search_reserves_them_with_its_hits() {
    let shard = shard();
    let at = |body: &str| hits_bytes(&parse_chunks([body.as_bytes()]).expect("parses"), &shard);
    let plain = at(r#"{"query":{"match_all":{}}}"#);
    assert_eq!(at(&query(NESTED)), plain + ONE);
}

/// One of the engine's document lists: four bytes for each of the 18 documents.
const LIST: usize = 4 * 18;

/// What one more `nested` aggregation reserves beside its bit set and lists: the
/// aggregation's own state, from `hits_bytes` before the lists were counted.
const OTHER: usize = 644 - ONE;

fn lists(shard: &OpenShard<Emulator>, body: &str) -> usize {
    list_bytes(&parse_chunks([body.as_bytes()]).expect("parses"), shard)
}

// AC-5
#[test]
fn a_nested_aggregation_reserves_the_engines_document_lists() {
    let shard = shard();
    assert_eq!(lists(&shard, r#"{"size":3}"#), 0);
    assert_eq!(
        lists(&shard, &query(NESTED)),
        0,
        "a query builds bit sets only"
    );
    assert_eq!(
        lists(&shard, r#"{"aggs":{"a":{"nested":{"path":"c"}}}}"#),
        LIST
    );
    let under = r#"{"aggs":{"a":{"nested":{"path":"c"},"aggs":{"r":{"reverse_nested":{}}}}}}"#;
    assert_eq!(lists(&shard, under), 2 * LIST);
    let beside = r#"{"aggs":{"a":{"nested":{"path":"c"}},"b":{"nested":{"path":"c"}}}}"#;
    assert_eq!(lists(&shard, beside), 2 * LIST);
}

// AC-5
#[test]
fn a_search_reserves_the_lists_with_its_hits() {
    let shard = shard();
    let at = |body: &str| hits_bytes(&parse_chunks([body.as_bytes()]).expect("parses"), &shard);
    let one = at(r#"{"aggs":{"a":{"nested":{"path":"c"}}}}"#);
    let two = at(r#"{"aggs":{"a":{"nested":{"path":"c"}},"b":{"nested":{"path":"c"}}}}"#);
    // The second one adds its bit set and its lists, beside the engine's state.
    assert_eq!(two - one, ONE + LIST + OTHER, "{one} {two}");
}
