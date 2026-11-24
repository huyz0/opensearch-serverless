//! `_ingest/pipeline` over a node's document set (docs/specs/pipeline-routes/SPEC.md).

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_node::pipeline_route::{PipelineService, routes};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const SET: &str = r#"{"description":"d","processors":[{"set":{"field":"a","value":1}}]}"#;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(PipelineService::new(&blobs, "", BUF).expect("service"));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

fn text(r: &support::Reply) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

#[test]
fn a_pipeline_is_stored_read_listed_and_removed() {
    let r = rig();
    let put = call(&r, "PUT", "/_ingest/pipeline/p1", Some(SET));
    assert_eq!(
        (put.status, text(&put)),
        (200, r#"{"acknowledged":true}"#.to_owned())
    );
    assert_eq!(
        call(&r, "POST", "/_ingest/pipeline/p2", Some(SET)).status,
        200
    );
    assert_eq!(
        text(&call(&r, "GET", "/_ingest/pipeline/p1", None)),
        format!(r#"{{"p1":{SET}}}"#)
    );
    let all = text(&call(&r, "GET", "/_ingest/pipeline", None));
    assert_eq!(all, format!(r#"{{"p1":{SET},"p2":{SET}}}"#));
    assert_eq!(text(&call(&r, "GET", "/_ingest/pipeline/*", None)), all);
    assert_eq!(
        call(
            &r,
            "PUT",
            "/_ingest/pipeline/p1",
            Some(r#"{"processors":[]}"#)
        )
        .status,
        200
    );
    assert_eq!(
        text(&call(&r, "GET", "/_ingest/pipeline/p1", None)),
        r#"{"p1":{"processors":[]}}"#
    );
    assert_eq!(call(&r, "DELETE", "/_ingest/pipeline/p1", None).status, 200);
    let gone = call(&r, "DELETE", "/_ingest/pipeline/p1", None);
    assert_eq!(gone.status, 404);
    assert!(text(&gone).contains("pipeline_missing"));
    assert_eq!(call(&r, "GET", "/_ingest/pipeline/p1", None).status, 404);
}

#[test]
fn what_the_node_cannot_run_is_refused_and_stores_nothing() {
    let r = rig();
    let script = r#"{"processors":[{"script":{"source":"x"}}]}"#;
    let refused = call(&r, "PUT", "/_ingest/pipeline/p", Some(script));
    assert_eq!(refused.status, 400);
    assert!(
        text(&refused).contains("Available processors"),
        "{}",
        text(&refused)
    );
    assert_eq!(call(&r, "PUT", "/_ingest/pipeline/p", None).status, 400);
    assert_eq!(
        call(&r, "PUT", "/_ingest/pipeline/_simulate", Some(SET)).status,
        501
    );
    assert_eq!(call(&r, "GET", "/_ingest/pipeline/a,b", None).status, 501);
    assert_eq!(call(&r, "GET", "/_ingest/pipeline/p", None).status, 404);
}

#[test]
fn a_read_of_an_unchanged_set_is_one_get_and_nothing_is_listed() {
    let r = rig();
    assert_eq!(
        call(&r, "PUT", "/_ingest/pipeline/p1", Some(SET)).status,
        200
    );
    assert_eq!(call(&r, "GET", "/_ingest/pipeline", None).status, 200);
    let total = |r: &Rig| -> u64 { S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum() };
    let before = total(&r);
    assert_eq!(call(&r, "GET", "/_ingest/pipeline", None).status, 200);
    assert_eq!(total(&r) - before, 1, "one marker GET");
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}

const UPPER: &str = r#"{"processors":[{"uppercase":{"field":"a"}}]}"#;

// docs/specs/pipeline-simulate AC-3
#[test]
fn a_pipeline_is_simulated_inline_and_by_id_without_storing_anything() {
    let r = rig();
    assert_eq!(
        call(&r, "PUT", "/_ingest/pipeline/up", Some(UPPER)).status,
        200
    );
    let docs = r#""docs":[{"_source":{"a":"x"}},{"_id":"2","_source":{"b":1}}]"#;
    let by_id = call(
        &r,
        "POST",
        "/_ingest/pipeline/up/_simulate",
        Some(&format!("{{{docs}}}")),
    );
    assert_eq!(by_id.status, 200, "{}", text(&by_id));
    let json: serde_json::Value = serde_json::from_str(&text(&by_id)).expect("json");
    assert_eq!(json["docs"][0]["doc"]["_source"]["a"], "X");
    assert_eq!(
        json["docs"][1]["error"]["type"],
        "illegal_argument_exception"
    );
    let inline = format!(r#"{{"pipeline":{UPPER},{docs}}}"#);
    for method in ["POST", "GET"] {
        let reply = call(&r, method, "/_ingest/pipeline/_simulate", Some(&inline));
        assert_eq!(reply.status, 200, "{method}: {}", text(&reply));
        assert!(text(&reply).contains(r#""a":"X""#), "{}", text(&reply));
    }
    // A missing stored id, `verbose`, and a body that is wrong.
    let missing = call(
        &r,
        "POST",
        "/_ingest/pipeline/nope/_simulate",
        Some(&format!("{{{docs}}}")),
    );
    assert_eq!(missing.status, 404, "{}", text(&missing));
    let verbose = call(
        &r,
        "POST",
        "/_ingest/pipeline/up/_simulate?verbose=true",
        Some(&format!("{{{docs}}}")),
    );
    assert_eq!(verbose.status, 501, "{}", text(&verbose));
    assert_eq!(
        call(
            &r,
            "POST",
            "/_ingest/pipeline/_simulate",
            Some(r#"{"docs":[{"_source":{}}]}"#)
        )
        .status,
        400
    );
    assert_eq!(
        call(&r, "POST", "/_ingest/pipeline/up/_simulate", None).status,
        400
    );
    // Nothing was stored: only `up` is held.
    let all = text(&call(&r, "GET", "/_ingest/pipeline", None));
    let held: serde_json::Value = serde_json::from_str(&all).expect("json");
    assert_eq!(held.as_object().expect("object").len(), 1, "{all}");
}

// docs/specs/pipeline-simulate AC-4
#[test]
fn a_simulation_reads_the_set_once_and_an_inline_one_not_at_all() {
    let r = rig();
    assert_eq!(
        call(&r, "PUT", "/_ingest/pipeline/up", Some(UPPER)).status,
        200
    );
    let body = r#"{"docs":[{"_source":{"a":"x"}}]}"#;
    assert_eq!(
        call(&r, "POST", "/_ingest/pipeline/up/_simulate", Some(body)).status,
        200
    );
    let total = |r: &Rig| -> u64 { S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum() };
    let before = total(&r);
    assert_eq!(
        call(&r, "POST", "/_ingest/pipeline/up/_simulate", Some(body)).status,
        200
    );
    assert_eq!(total(&r) - before, 1, "one marker GET");
    let inline = format!(r#"{{"pipeline":{UPPER},"docs":[{{"_source":{{"a":"x"}}}}]}}"#);
    let before = total(&r);
    assert_eq!(
        call(&r, "POST", "/_ingest/pipeline/_simulate", Some(&inline)).status,
        200
    );
    assert_eq!(total(&r), before, "an inline simulation makes no request");
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}
