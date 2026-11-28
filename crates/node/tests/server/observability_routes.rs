//! docs/specs/observability slice 1: create and read an object.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_node::observability_route::{ObservabilityService, routes};
use oss_rest::{Router, Server};
use serde_json::Value;

const BUF: usize = 1 << 16;
const QUERY: &str = r#"{"savedQuery":{"name":"errors","description":"","query":"source = logs | where level = 'error'","selected_date_range":{"start":"now-1d","end":"now","text":""},"selected_timestamp":{"name":"@timestamp","type":"timestamp"},"selected_fields":{"text":"","tokens":[]}}}"#;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(ObservabilityService::new(&blobs, "", BUF));
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

fn json(reply: &support::Reply) -> Value {
    serde_json::from_str(&reply.text()).unwrap_or_else(|e| panic!("{e}: {}", reply.text()))
}

const OBJECT: &str = "/_plugins/_observability/object";

// AC-1
#[test]
fn an_object_is_created_and_read_back_under_its_id() {
    let r = rig();
    let made = call(&r, "POST", OBJECT, Some(QUERY));
    assert_eq!(made.status, 200, "{}", made.text());
    let id = json(&made)["objectId"].as_str().expect("an id").to_owned();
    assert!(!id.is_empty());

    let read = call(&r, "GET", &format!("{OBJECT}/{id}"), None);
    assert_eq!(read.status, 200, "{}", read.text());
    let body = json(&read);
    assert_eq!(body["totalHits"], 1);
    let object = &body["observabilityObjectList"][0];
    assert_eq!(object["objectId"], id);
    assert_eq!(object["savedQuery"]["name"], "errors");
    assert_eq!(object["tenant"], "");
    assert!(object["createdTimeMs"].as_i64().unwrap() > 0);
    assert_eq!(object["createdTimeMs"], object["lastUpdatedTimeMs"]);

    let other = call(&r, "POST", OBJECT, Some(QUERY));
    assert_ne!(json(&other)["objectId"], id, "an id is never reused");
}

// AC-1
#[test]
fn a_missing_object_is_a_404() {
    let r = rig();
    let read = call(&r, "GET", &format!("{OBJECT}/nope"), None);
    assert_eq!(read.status, 404, "{}", read.text());
    assert_eq!(json(&read)["error"]["type"], "resource_not_found_exception");
}

// AC-2
#[test]
fn a_body_that_is_not_one_known_object_is_a_400() {
    let r = rig();
    for body in [
        "not json",
        "[]",
        "{}",
        r#"{"mystery":{}}"#,
        r#"{"savedQuery":"text"}"#,
        r#"{"savedQuery":{"name":7}}"#,
        r#"{"savedQuery":{},"savedVisualization":{}}"#,
    ] {
        let reply = call(&r, "POST", OBJECT, Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.text());
    }
    let long = format!(r#"{{"savedQuery":{{"name":"{}"}}}}"#, "n".repeat(257));
    assert_eq!(call(&r, "POST", OBJECT, Some(&long)).status, 400);
}

// AC-2
#[test]
fn an_object_over_a_mebibyte_is_refused_before_it_is_stored() {
    let r = rig();
    let big = format!(
        r#"{{"savedNotebook":{{"name":"n","pad":"{}"}}}}"#,
        "x".repeat(1 << 20)
    );
    let reply = call(&r, "POST", OBJECT, Some(&big));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert_eq!(r.store.meter().count(S3Op::Put), 0);
}

// AC-2
#[test]
fn every_object_type_the_plugin_stores_is_accepted() {
    let r = rig();
    for ty in [
        "savedQuery",
        "savedVisualization",
        "operationalPanel",
        "savedNotebook",
        "timestamp",
        "integration",
        "observabilityDashboard",
    ] {
        let body = format!(r#"{{"{ty}":{{"name":"x"}}}}"#);
        let reply = call(&r, "POST", OBJECT, Some(&body));
        assert_eq!(reply.status, 200, "{ty}: {}", reply.text());
    }
}

// AC-6
#[test]
fn a_create_costs_one_get_and_two_puts_a_read_two_gets_and_nothing_lists() {
    let r = rig();
    let m = &r.store;
    let (gets, puts) = (m.meter().count(S3Op::Get), m.meter().count(S3Op::Put));
    let made = call(&r, "POST", OBJECT, Some(QUERY));
    assert_eq!(
        (
            m.meter().count(S3Op::Get) - gets,
            m.meter().count(S3Op::Put) - puts
        ),
        (1, 2),
        "a create"
    );
    let id = json(&made)["objectId"].as_str().unwrap().to_owned();
    let gets = m.meter().count(S3Op::Get);
    call(&r, "GET", &format!("{OBJECT}/{id}"), None);
    assert_eq!(m.meter().count(S3Op::Get) - gets, 2, "a read");
    assert_eq!(m.meter().count(S3Op::List), 0);
}
