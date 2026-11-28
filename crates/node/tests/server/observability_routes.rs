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
    assert_eq!(json(&read)["error"]["type"], "status_exception");
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
        "application",
        "operationalPanel",
        "notebook",
    ] {
        let body = format!(r#"{{"{ty}":{{"name":"x"}}}}"#);
        let reply = call(&r, "POST", OBJECT, Some(&body));
        assert_eq!(reply.status, 200, "{ty}: {}", reply.text());
    }
}

// AC-2: the stock plugin does not know these types
#[test]
fn the_types_the_stock_plugin_lacks_are_refused() {
    let r = rig();
    for ty in [
        "savedNotebook",
        "timestamp",
        "timestampBased",
        "integration",
    ] {
        let body = format!(r#"{{"{ty}":{{"name":"x"}}}}"#);
        let reply = call(&r, "POST", OBJECT, Some(&body));
        assert_eq!(reply.status, 400, "{ty}: {}", reply.text());
        assert_eq!(
            json(&reply)["error"]["reason"],
            "Object type field absent",
            "{ty}"
        );
    }
}

// AC-1
#[test]
fn replace_delete_and_list_work_over_a_few_objects() {
    let r = rig();
    for (id, name) in [("a", "zed"), ("b", "alpha"), ("c", "mid")] {
        let body = format!(r#"{{"objectId":"{id}","savedQuery":{{"name":"{name}"}}}}"#);
        assert_eq!(call(&r, "POST", OBJECT, Some(&body)).status, 200);
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    let names = |q: &str| -> Vec<String> {
        let reply = call(&r, "GET", &format!("{OBJECT}{q}"), None);
        assert_eq!(reply.status, 200, "{}", reply.text());
        json(&reply)["observabilityObjectList"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["savedQuery"]["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(names(""), ["zed", "alpha", "mid"]);
    assert_eq!(names("?sortField=name"), ["alpha", "mid", "zed"]);
    assert_eq!(
        names("?sortField=name&sortOrder=desc&maxItems=2"),
        ["zed", "mid"]
    );
    assert_eq!(names("?fromIndex=1&maxItems=1&sortField=name"), ["mid"]);
    assert_eq!(names("?name=mid"), ["mid"]);

    let put = |id: &str, body: &str| call(&r, "PUT", &format!("{OBJECT}/{id}"), Some(body));
    assert_eq!(put("a", r#"{"savedQuery":{"name":"renamed"}}"#).status, 200);
    assert_eq!(put("a", r#"{"application":{"name":"x"}}"#).status, 409);
    assert_eq!(put("zz", r#"{"savedQuery":{"name":"x"}}"#).status, 404);
    assert_eq!(names("?sortField=name"), ["alpha", "mid", "renamed"]);

    let gone = call(&r, "DELETE", &format!("{OBJECT}?objectIdList=a,nope"), None);
    assert_eq!(gone.status, 404, "{}", gone.text());
    assert_eq!(names("").len(), 3, "a missing id deletes nothing");
    let gone = call(&r, "DELETE", &format!("{OBJECT}?objectIdList=a,b"), None);
    assert_eq!(json(&gone)["deleteResponseList"]["a"], "OK");
    assert_eq!(names(""), ["mid"]);
    assert_eq!(call(&r, "DELETE", &format!("{OBJECT}/c"), None).status, 200);
    assert_eq!(call(&r, "DELETE", &format!("{OBJECT}/c"), None).status, 404);
}

// AC-1
#[test]
fn a_supplied_id_is_never_overwritten_by_a_create() {
    let r = rig();
    let body = r#"{"objectId":"fixed","savedQuery":{"name":"one"}}"#;
    assert_eq!(call(&r, "POST", OBJECT, Some(body)).status, 200);
    let again = call(&r, "POST", OBJECT, Some(body));
    assert_eq!(again.status, 409, "{}", again.text());
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
    let gets = m.meter().count(S3Op::Get);
    call(&r, "GET", OBJECT, None);
    assert_eq!(m.meter().count(S3Op::Get) - gets, 1 + 1, "a list of one");
    assert_eq!(m.meter().count(S3Op::List), 0);
}
