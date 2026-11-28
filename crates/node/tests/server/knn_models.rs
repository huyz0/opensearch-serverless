//! docs/specs/knn-plugin-api slice 3: the model store's routes.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::DocKind;
use oss_mem::MemoryBudget;
use oss_meta::DocSet;
use oss_node::knn_model_route::{KnnModelService, routes};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

const BUF: usize = 1 << 16;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(KnnModelService::new(&blobs, "", BUF));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

fn model(id: &str, state: &str) -> Value {
    json!({"model_id": id, "model_blob": "AAEC", "state": state,
           "timestamp": "2026-11-28T00:00:00Z", "description": "d", "error": "",
           "space_type": "l2", "dimension": 4, "engine": "faiss"})
}

fn put(r: &Rig, id: &str, state: &str) {
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let set = DocSet::new(blobs, "", DocKind::KnnModels).expect("set");
    let mut marker = vec![0u8; BUF];
    set.put_counted(
        id,
        model(id, state).to_string().as_bytes(),
        false,
        &mut marker,
    )
    .expect("put");
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

fn json_of(reply: &support::Reply) -> Value {
    serde_json::from_str(&reply.text()).expect("json")
}

// AC-10
#[test]
fn a_stored_model_is_read_as_its_source_and_a_missing_one_is_404() {
    let r = rig();
    put(&r, "m1", "created");
    let got = call(&r, "GET", "/_plugins/_knn/models/m1", None);
    assert_eq!(got.status, 200, "{}", got.text());
    assert_eq!(json_of(&got), model("m1", "created"));
    let none = call(&r, "GET", "/_plugins/_knn/models/nope", None);
    assert_eq!(none.status, 404, "{}", none.text());
    assert!(none.text().contains("resource_not_found_exception"));
    assert!(
        none.text().contains(r#"Model \"nope\" does not exist"#),
        "{}",
        none.text()
    );
}

// AC-11
#[test]
fn a_delete_removes_a_model_and_refuses_one_that_is_training() {
    let r = rig();
    put(&r, "done", "created");
    put(&r, "busy", "training");
    let gone = call(&r, "DELETE", "/_plugins/_knn/models/done", None);
    assert_eq!(gone.status, 200, "{}", gone.text());
    assert_eq!(
        call(&r, "GET", "/_plugins/_knn/models/done", None).status,
        404
    );
    assert_eq!(
        call(&r, "DELETE", "/_plugins/_knn/models/done", None).status,
        404
    );
    let busy = call(&r, "DELETE", "/_plugins/_knn/models/busy", None);
    assert_eq!(busy.status, 400, "{}", busy.text());
    assert_eq!(
        call(&r, "GET", "/_plugins/_knn/models/busy", None).status,
        200
    );
}

// AC-12
#[test]
fn a_search_pages_models_in_id_order_and_serves_match_all_and_ids_only() {
    let r = rig();
    for id in ["c", "a", "b"] {
        put(&r, id, "created");
    }
    let all = json_of(&call(
        &r,
        "POST",
        "/_plugins/_knn/models/_search",
        Some("{}"),
    ));
    assert_eq!(all["hits"]["total"]["value"], 3);
    let ids: Vec<&str> = all["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["_id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(all["hits"]["hits"][0]["_index"], ".opensearch-knn-models");
    assert_eq!(all["hits"]["hits"][0]["_source"], model("a", "created"));
    let page = json_of(&call(
        &r,
        "GET",
        "/_plugins/_knn/models/_search?from=1&size=1",
        None,
    ));
    assert_eq!(page["hits"]["hits"].as_array().expect("hits").len(), 1);
    assert_eq!(page["hits"]["hits"][0]["_id"], "b");
    let by_ids = json_of(&call(
        &r,
        "POST",
        "/_plugins/_knn/models/_search",
        Some(r#"{"query":{"ids":{"values":["c","zz"]}}}"#),
    ));
    assert_eq!(by_ids["hits"]["total"]["value"], 1);
    let refused = call(
        &r,
        "POST",
        "/_plugins/_knn/models/_search",
        Some(r#"{"query":{"match":{"state":"created"}}}"#),
    );
    assert_eq!(refused.status, 400, "{}", refused.text());
}

// AC-13
#[test]
fn a_read_costs_the_marker_and_the_content_and_nothing_lists() {
    let r = rig();
    put(&r, "m1", "created");
    let before = (
        r.store.meter().count(S3Op::Get),
        r.store.meter().count(S3Op::List),
    );
    assert_eq!(
        call(&r, "GET", "/_plugins/_knn/models/m1", None).status,
        200
    );
    let after = (
        r.store.meter().count(S3Op::Get),
        r.store.meter().count(S3Op::List),
    );
    assert_eq!(after.0 - before.0, 2, "marker GET and content GET");
    assert_eq!(after.1, before.1, "no LIST");
}
