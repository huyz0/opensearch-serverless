//! docs/specs/alerting slice 2: the monitor routes over a tenant's index.

use crate::support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator};
use oss_mem::MemoryBudget;
use oss_meta::AlertRegistry;
use oss_node::alerting_route::{AlertingService, routes};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(AlertingService::new(&blobs, ""));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

/// A request with an optional `securitytenant` header; the status and body.
fn send(
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
        "inputs": [{"search": {"indices": ["logs"], "query": {"size": 0, "query": {"match_all": {}}}}}],
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
    assert_eq!((status, gone["result"].as_str()), (200, Some("deleted")));
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
    cases.push(("monitor_type", kind));
    let mut fast = monitor("m", true);
    fast["schedule"] = json!({"period": {"interval": 0, "unit": "MINUTES"}});
    cases.push(("interval", fast));
    let mut unit = monitor("m", true);
    unit["schedule"] = json!({"period": {"interval": 1, "unit": "SECONDS"}});
    cases.push(("unit", unit));
    let mut none = monitor("m", true);
    none["inputs"] = json!([]);
    cases.push(("inputs", none));
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
