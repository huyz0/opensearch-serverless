//! docs/specs/knn-plugin-api slice 1: stats, warmup and clear_cache.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use lucene_codecs::field_infos::VectorSimilarityFunction;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, S3Op};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::search::{SearchService, knn_api, routes};
use oss_rest::{Router, Server};
use oss_testkit::index::Spec;

struct Rig {
    server: Server,
    service: Arc<SearchService<oss_blob::Emulator>>,
    store: Arc<oss_blob::PooledStore<oss_blob::Emulator>>,
    open: MemoryBudget,
}

fn rig(knn_routes: bool) -> Rig {
    let store = support::store();
    support::put_built(
        &store,
        &Spec::new("vec", 40)
            .segments_of(15)
            .with_vectors(VectorSimilarityFunction::Euclidean),
    );
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    let open = MemoryBudget::new(
        &BudgetConfig::new(16 << 20)
            .with_pool(Pool::OpenIndexes, 16 << 20)
            .expect("fits"),
    );
    let service = Arc::new(SearchService::new(
        Arc::clone(&store),
        cache,
        "",
        support::NODE,
        &open,
    ));
    let mut router = routes(Router::new(), Arc::clone(&service)).expect("routes");
    if knn_routes {
        router = knn_api::routes(router, &service).expect("knn routes");
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        service,
        store,
        open,
    }
}

fn get(r: &Rig, target: &str) -> support::Reply {
    support::request(r.server.local_addr(), "GET", target, None)
}

fn knn_search(r: &Rig, extra: &str) -> support::Reply {
    let body = format!(r#"{{"query":{{"knn":{{"v":{{"vector":[1,2,3],"k":3{extra}}}}}}}}}"#);
    support::request(
        r.server.local_addr(),
        "POST",
        "/vec/_search",
        Some(body.as_bytes()),
    )
}

fn number(body: &str, key: &str) -> u64 {
    let v: serde_json::Value = serde_json::from_str(body).expect("json");
    v["nodes"][support::NODE][key].as_u64().expect(key)
}

// AC-1
#[test]
fn stats_answer_the_plugins_names_under_this_node() {
    let r = rig(true);
    let reply = get(&r, "/_plugins/_knn/stats");
    assert_eq!(reply.status, 200, "{}", reply.text());
    let v: serde_json::Value = serde_json::from_str(&reply.text()).expect("json");
    assert_eq!(v["_nodes"]["total"], 1);
    assert_eq!(v["circuit_breaker_triggered"], false);
    let node = &v["nodes"][support::NODE];
    for name in [
        "knn_query_requests",
        "knn_query_with_filter_requests",
        "graph_memory_usage",
        "hit_count",
        "faiss_initialized",
        "indices_in_cache",
    ] {
        assert!(node.get(name).is_some(), "{name} in {}", reply.text());
    }
    assert_eq!(node["faiss_initialized"], false);
}

// AC-2
#[test]
fn a_knn_search_is_counted_and_a_filtered_one_is_counted_as_filtered() {
    let r = rig(true);
    let before = get(&r, "/_plugins/_knn/stats").text();
    assert_eq!(knn_search(&r, "").status, 200);
    let mid = get(&r, "/_plugins/_knn/stats").text();
    assert!(number(&mid, "knn_query_requests") > number(&before, "knn_query_requests"));
    assert_eq!(knn_search(&r, r#","filter":{"match_all":{}}"#).status, 200);
    let after = get(&r, "/_plugins/_knn/stats").text();
    assert!(
        number(&after, "knn_query_with_filter_requests")
            > number(&mid, "knn_query_with_filter_requests")
    );
    assert!(
        get(&r, "/_plugins/_knn/stats")
            .text()
            .contains(r#""lucene_initialized":true"#)
    );
}

// AC-3
#[test]
fn a_named_stat_answers_alone_an_unknown_one_is_400_and_another_node_is_404() {
    let r = rig(true);
    let one = get(&r, "/_plugins/_knn/stats/hit_count");
    assert_eq!(one.status, 200, "{}", one.text());
    let v: serde_json::Value = serde_json::from_str(&one.text()).expect("json");
    assert_eq!(v["nodes"][support::NODE].as_object().expect("obj").len(), 1);
    assert_eq!(get(&r, "/_plugins/_knn/stats/nope").status, 400);
    let local = get(&r, &format!("/_plugins/_knn/{}/stats", support::NODE));
    assert_eq!(local.status, 200, "{}", local.text());
    assert_eq!(get(&r, "/_plugins/_knn/other-node/stats").status, 404);
}

// AC-4
#[test]
fn warmup_opens_every_shard_and_an_unknown_index_is_404() {
    let r = rig(true);
    let ok = get(&r, "/_plugins/_knn/warmup/vec");
    assert_eq!(ok.status, 200, "{}", ok.text());
    let v: serde_json::Value = serde_json::from_str(&ok.text()).expect("json");
    assert_eq!(
        v["_shards"],
        serde_json::json!({"total":1,"successful":1,"failed":0})
    );
    assert_eq!(r.service.open_indexes(), 1);
    assert_eq!(get(&r, "/_plugins/_knn/warmup/vec").status, 200);
    assert_eq!(r.service.open_indexes(), 1);
    assert_eq!(get(&r, "/_plugins/_knn/warmup/missing").status, 404);
}

// AC-5, AC-6
#[test]
fn clear_cache_closes_the_open_shards_returns_the_charge_and_asks_for_nothing() {
    let r = rig(true);
    assert_eq!(knn_search(&r, "").status, 200);
    assert_eq!(r.service.open_indexes(), 1);
    assert!(r.open.in_use(Pool::OpenIndexes) > 0);
    let gets = r.store.store().meter().count(S3Op::Get);
    let cleared = get(&r, "/_plugins/_knn/clear_cache/vec");
    assert_eq!(cleared.status, 200, "{}", cleared.text());
    assert!(cleared.text().contains(r#""successful":1"#));
    assert_eq!(r.service.open_indexes(), 0);
    assert_eq!(r.open.in_use(Pool::OpenIndexes), 0);
    assert_eq!(knn_search(&r, "").status, 200, "opens afresh");
    assert_eq!(r.service.open_indexes(), 1);
    assert!(r.store.store().meter().count(S3Op::Get) > gets);
    let stats_gets = r.store.store().meter().count(S3Op::Get);
    assert_eq!(get(&r, "/_plugins/_knn/stats").status, 200);
    assert_eq!(r.store.store().meter().count(S3Op::Get), stats_gets);
    assert_eq!(get(&r, "/_plugins/_knn/clear_cache/missing").status, 404);
}

// AC-7
#[test]
fn without_the_routes_every_path_is_an_unknown_route() {
    let r = rig(false);
    assert_eq!(get(&r, "/_plugins/_knn/stats").status, 400);
    assert_eq!(get(&r, "/_plugins/_knn/warmup/vec").status, 400);
}
