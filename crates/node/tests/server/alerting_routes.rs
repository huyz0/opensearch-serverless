//! docs/specs/alerting slice 2: the monitor routes over a tenant's index.

use crate::support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_meta::{AlertRegistry, DeleteMode, DescriptorStore, PitClock};
use oss_node::alerting_route::{AlertingService, routes};
use oss_node::index_route::{IndexService, routes as index_routes};
use oss_rest::{Router, Server};
use oss_testkit::budget;
use serde_json::{Value, json};

pub(crate) struct Rig {
    pub(crate) server: Server,
    pub(crate) store: Arc<Emulator>,
}

pub(crate) fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let descriptors =
        || DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).expect("names");
    let service =
        Arc::new(AlertingService::new(&blobs, "").with_descriptors(descriptors(), 1 << 16));
    let clock: PitClock = Arc::new(|| 1_234_567);
    let indices = Arc::new(IndexService::new(descriptors(), clock, 1 << 16));
    let router = index_routes(Router::new(), indices).expect("routes");
    let router = routes(router, &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    let r = Rig { server, store };
    let (status, _) = send(
        r.server.local_addr(),
        "PUT",
        "/compat-alert-src",
        None,
        None,
    );
    assert_eq!(status, 200);
    r
}

/// A request with an optional `securitytenant` header; the status and body.
pub(crate) fn send(
    addr: SocketAddr,
    method: &str,
    target: &str,
    tenant: Option<&str>,
    body: Option<&str>,
) -> (u16, Value) {
    let mut s = TcpStream::connect(addr).expect("connects");
    let mut head = format!("{method} {target} HTTP/1.1\r\nhost: test\r\nconnection: close\r\n");
    if let Some(t) = tenant {
        head.push_str(&format!("securitytenant: {t}\r\n"));
    }
    if let Some(b) = body {
        head.push_str(&format!(
            "content-type: application/json\r\ncontent-length: {}\r\n",
            b.len()
        ));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).expect("sends");
    if let Some(b) = body {
        s.write_all(b.as_bytes()).expect("sends body");
    }
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text[9..12].parse().expect("status");
    let body = text.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&Value>) -> (u16, Value) {
    send(
        r.server.local_addr(),
        method,
        target,
        None,
        body.map(Value::to_string).as_deref(),
    )
}

fn monitor(name: &str, enabled: bool) -> Value {
    json!({
        "type": "monitor", "name": name, "monitor_type": "query_level_monitor",
        "enabled": enabled,
        "schedule": {"period": {"interval": 5, "unit": "MINUTES"}},
        "inputs": [{"search": {"indices": ["compat-alert-src"], "query": {"size": 0, "query": {"match_all": {}}}}}],
        "triggers": [{"query_level_trigger": {"name": "t", "severity": "1",
            "condition": {"script": {"source": "ctx.results[0].hits.total.value > 0", "lang": "painless"}},
            "actions": []}}]
    })
}

const BASE: &str = "/_plugins/_alerting/monitors";

#[test]
fn a_monitor_is_created_read_replaced_and_deleted_on_both_prefixes() {
    let r = rig();
    let (status, made) = call(&r, "POST", BASE, Some(&monitor("m", true)));
    assert_eq!(status, 201, "{made}");
    let id = made["_id"].as_str().expect("an id").to_owned();
    assert_eq!(made["monitor"]["name"], "m");
    assert_eq!(made["monitor"]["type"], "monitor");
    assert!(made["monitor"]["enabled_time"].is_i64());
    for prefix in ["/_plugins/_alerting", "/_opendistro/_alerting"] {
        let (status, got) = call(&r, "GET", &format!("{prefix}/monitors/{id}"), None);
        assert_eq!(status, 200, "{got}");
        assert_eq!(got["_id"], id.as_str());
        assert_eq!(got["monitor"]["name"], "m");
    }
    let (status, put) = call(
        &r,
        "PUT",
        &format!("{BASE}/{id}"),
        Some(&monitor("renamed", false)),
    );
    assert_eq!(status, 200, "{put}");
    assert!(put["_seq_no"].as_i64() > made["_seq_no"].as_i64());
    let (_, got) = call(&r, "GET", &format!("{BASE}/{id}"), None);
    assert_eq!(got["monitor"]["name"], "renamed");
    assert_eq!(got["monitor"]["enabled"], false);
    assert!(got["monitor"]["enabled_time"].is_null());
    let (status, gone) = call(&r, "DELETE", &format!("{BASE}/{id}"), None);
    assert_eq!((status, gone["_id"].as_str()), (200, Some(id.as_str())));
    assert_eq!(call(&r, "GET", &format!("{BASE}/{id}"), None).0, 404);
    assert_eq!(call(&r, "DELETE", &format!("{BASE}/{id}"), None).0, 404);
    assert_eq!(
        call(
            &r,
            "PUT",
            &format!("{BASE}/{id}"),
            Some(&monitor("x", true))
        )
        .0,
        404
    );
}

#[test]
fn an_invalid_monitor_is_a_400_and_stores_nothing() {
    let r = rig();
    let mut cases: Vec<(&str, Value)> = Vec::new();
    let mut no_name = monitor("m", true);
    no_name.as_object_mut().unwrap().remove("name");
    cases.push(("name", no_name));
    let mut kind = monitor("m", true);
    kind["monitor_type"] = json!("nonsense");
    let (status, answer) = call(&r, "POST", BASE, Some(&kind));
    assert_eq!(status, 500, "monitor_type: {answer}");
    let mut fast = monitor("m", true);
    fast["schedule"] = json!({"period": {"interval": 0, "unit": "MINUTES"}});
    cases.push(("interval", fast));
    let mut unit = monitor("m", true);
    unit["schedule"] = json!({"period": {"interval": 1, "unit": "SECONDS"}});
    cases.push(("unit", unit));
    let mut many = monitor("m", true);
    many["triggers"] = json!(vec![json!({"query_level_trigger": {"actions": []}}); 101]);
    cases.push(("triggers", many));
    let mut actions = monitor("m", true);
    actions["triggers"] = json!([{"query_level_trigger": {"actions": vec![json!({}); 101]}}]);
    cases.push(("actions", actions));
    for (what, body) in &cases {
        let (status, answer) = call(&r, "POST", BASE, Some(body));
        assert_eq!(status, 400, "{what}: {answer}");
    }
    let (status, _) = send(r.server.local_addr(), "POST", BASE, None, Some("{not json"));
    assert_eq!(status, 400);
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let monitors = oss_meta::Monitors::new(&blobs, "", "_default_").unwrap();
    assert!(monitors.index().unwrap().is_empty());
}

#[test]
fn a_schedule_below_the_minimum_is_refused_and_the_floor_is_exact() {
    let r = rig();
    let mut at_floor = monitor("m", true);
    at_floor["schedule"] = json!({"period": {"interval": 1, "unit": "MINUTES"}});
    assert_eq!(call(&r, "POST", BASE, Some(&at_floor)).0, 201);
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let strict = Arc::new(AlertingService::new(&blobs, "").with_min_interval(61));
    let router = routes(Router::new(), &strict).expect("routes");
    let budget = MemoryBudget::new(&support::budget_config());
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &budget,
    )
    .expect("starts");
    let (status, _) = send(
        server.local_addr(),
        "POST",
        BASE,
        None,
        Some(&at_floor.to_string()),
    );
    assert_eq!(status, 400);
}

#[test]
fn tenants_are_separate_and_an_enabled_monitor_registers_its_tenant() {
    let r = rig();
    let addr = r.server.local_addr();
    let body = monitor("m", true).to_string();
    let (status, made) = send(addr, "POST", BASE, Some("acme"), Some(&body));
    assert_eq!(status, 201, "{made}");
    let id = made["_id"].as_str().unwrap().to_owned();
    let path = format!("{BASE}/{id}");
    assert_eq!(send(addr, "GET", &path, Some("acme"), None).0, 200);
    assert_eq!(send(addr, "GET", &path, Some("zenith"), None).0, 404);
    assert_eq!(send(addr, "GET", &path, None, None).0, 404);
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let registry = AlertRegistry::new(blobs, "").unwrap();
    assert!(registry.is_registered("acme").unwrap());
    assert!(!registry.is_registered("zenith").unwrap());
    assert_eq!(send(addr, "DELETE", &path, Some("acme"), None).0, 200);
    assert!(!registry.is_registered("acme").unwrap());
    assert_eq!(
        send(addr, "POST", BASE, Some("a/b"), Some(&body)).0,
        400,
        "a tenant is one path segment"
    );
}

fn named(r: &Rig, name: &str, enabled: bool) -> String {
    let (status, made) = call(r, "POST", BASE, Some(&monitor(name, enabled)));
    assert_eq!(status, 201, "{made}");
    made["_id"].as_str().expect("an id").to_owned()
}

fn names(answer: &Value) -> Vec<&str> {
    answer["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["_source"]["name"].as_str().expect("a name"))
        .collect()
}

#[test]
fn a_search_filters_sorts_and_pages_a_tenants_monitors() {
    let r = rig();
    for (name, enabled) in [("b", false), ("a", false), ("c", true), ("d", false)] {
        named(&r, name, enabled);
    }
    let search = |body: Value| call(&r, "POST", &format!("{BASE}/_search"), Some(&body));
    let (status, all) =
        search(json!({"query": {"match_all": {}}, "sort": [{"monitor.name.keyword": "asc"}]}));
    assert_eq!(status, 200, "{all}");
    assert_eq!(names(&all), ["a", "b", "c", "d"]);
    assert_eq!(all["hits"]["total"], json!({"value": 4, "relation": "eq"}));
    assert_eq!(all["hits"]["hits"][0]["sort"], json!(["a"]));
    let (_, desc) = search(
        json!({"sort": [{"monitor.name.keyword": {"order": "desc"}}], "from": 1, "size": 2}),
    );
    assert_eq!(names(&desc), ["c", "b"]);
    assert_eq!(desc["hits"]["total"]["value"], 4);
    let (_, one) = search(json!({"query": {"term": {"monitor.name.keyword": "b"}}}));
    assert_eq!(names(&one), ["b"]);
    let (_, on) =
        search(json!({"query": {"bool": {"filter": [{"term": {"monitor.enabled": true}}]}}}));
    assert_eq!(names(&on), ["c"]);
    let (_, off) = search(
        json!({"query": {"bool": {"must_not": [{"term": {"monitor.enabled": true}}],
        "must": [{"prefix": {"monitor.name.keyword": "a"}}]}}}),
    );
    assert_eq!(names(&off), ["a"]);
    let (_, either) = search(json!({"query": {"bool": {"should": [
        {"terms": {"monitor.name.keyword": ["a", "d"]}}, {"wildcard": {"monitor.name.keyword": "c*"}}]}},
        "sort": ["monitor.name.keyword"]}));
    assert_eq!(names(&either), ["a", "c", "d"]);
    let (_, glob) =
        search(json!({"query": {"wildcard": {"monitor.name.keyword": "?"}}, "size": 0}));
    assert_eq!(glob["hits"]["total"]["value"], 4);
    let (_, glob) = search(json!({"query": {"wildcard": {"monitor.name.keyword": "*x*"}}}));
    assert_eq!(glob["hits"]["total"]["value"], 0);
    let (_, glob) = search(json!({"query": {"wildcard": {"monitor.name.keyword": "??"}}}));
    assert_eq!(glob["hits"]["total"]["value"], 0);
    let (_, none) = search(json!({"query": {"term": {"monitor.name.keyword": "zzz"}}}));
    assert_eq!(none["hits"]["total"]["value"], 0);
    let (_, count) = search(json!({"size": 0, "query": {"match_all": {}}}));
    assert_eq!(
        (
            count["hits"]["total"]["value"].as_i64(),
            names(&count).len()
        ),
        (Some(4), 0)
    );
    let (status, other) = send(
        r.server.local_addr(),
        "POST",
        &format!("{BASE}/_search"),
        Some("other"),
        Some("{}"),
    );
    assert_eq!(
        (status, other["hits"]["total"]["value"].as_i64()),
        (200, Some(0))
    );
}

#[test]
fn a_search_without_a_body_or_with_an_unknown_clause_is_a_400() {
    let r = rig();
    named(&r, "a", false);
    for method in ["GET", "POST"] {
        let (status, e) = call(&r, method, &format!("{BASE}/_search"), None);
        assert_eq!(
            (status, e["error"]["type"].as_str()),
            (400, Some("parse_exception")),
            "{e}"
        );
    }
    let (status, _) = call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(&json!({"query": {"fuzzy": {}}})),
    );
    assert_eq!(status, 400);
    let (status, _) = call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(&json!({"size": 10_001})),
    );
    assert_eq!(status, 400);
}

#[test]
fn a_search_reads_only_the_page_it_returns_unless_it_must_look_at_every_body() {
    let r = rig();
    for name in ["a", "b", "c", "d"] {
        named(&r, name, false);
    }
    let gets = || r.store.meter().count(S3Op::Get);
    let at = gets();
    let (status, _) = call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(&json!({"size": 1})),
    );
    assert_eq!(status, 200);
    budget::check(
        "alerting.monitor_search_page.requests",
        gets() - at,
        "a page of one over four monitors",
    );
    let at = gets();
    let (status, _) = call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(&json!({"query": {"term": {"monitor.name.keyword": "c"}}})),
    );
    assert_eq!(status, 200);
    budget::check(
        "alerting.monitor_search_filtered.requests",
        gets() - at,
        "a filtered search over four monitors",
    );
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}
