//! docs/specs/alias-routes: add, remove and read the aliases of named indices.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore, PitClock};
use oss_node::alias_route::{AliasService, routes};
use oss_node::index_route::{IndexService, routes as index_routes};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const TTL: i64 = 30_000;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let clock: PitClock = Arc::new(|| 1_234_567);
    let descriptors = || {
        DescriptorStore::new(
            Arc::clone(&store) as Arc<dyn BlobStore>,
            "",
            DeleteMode::Conditional,
        )
        .expect("names")
    };
    let indices = Arc::new(IndexService::new(descriptors(), Arc::clone(&clock), BUF));
    let aliases = Arc::new(AliasService::new(descriptors(), clock, BUF, TTL));
    let router = index_routes(Router::new(), indices).expect("routes");
    let router = routes(router, aliases).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    let r = Rig { server, store };
    for name in ["a", "b", "c"] {
        assert_eq!(call(&r, "PUT", &format!("/{name}"), None).status, 200);
    }
    r
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

const ACK: &str = r#"{"acknowledged":true}"#;

fn members(r: &Rig, alias: &str) -> serde_json::Value {
    let reply = call(r, "GET", &format!("/_alias/{alias}"), None);
    assert_eq!(reply.status, 200, "{}", reply.text());
    serde_json::from_str(&reply.text()).expect("json")
}

// AC-1
#[test]
fn an_add_creates_the_alias_and_it_reads_back() {
    let r = rig();
    let reply = call(&r, "PUT", "/a/_alias/all", None);
    assert_eq!((reply.status, reply.text().as_str()), (200, ACK));
    assert_eq!(
        members(&r, "all"),
        serde_json::json!({"a": {"aliases": {"all": {}}}})
    );
    assert_eq!(call(&r, "HEAD", "/_alias/all", None).status, 200);
    assert_eq!(call(&r, "HEAD", "/_alias/none", None).status, 404);
    let missing = call(&r, "GET", "/_alias/none", None);
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.text(),
        r#"{"error":"alias [none] missing","status":404}"#
    );
    assert_eq!(call(&r, "POST", "/a/_alias/all", None).status, 200);
}

// AC-2
#[test]
fn a_second_index_extends_the_alias_and_the_write_index_moves() {
    let r = rig();
    assert_eq!(
        call(&r, "PUT", "/a/_alias/w", Some(r#"{"is_write_index":true}"#)).status,
        200
    );
    assert_eq!(call(&r, "PUT", "/b/_alias/w", None).status, 200);
    let got = members(&r, "w");
    assert_eq!(got["a"]["aliases"]["w"]["is_write_index"], true);
    assert!(got["b"]["aliases"]["w"].get("is_write_index").is_none());
    assert_eq!(
        call(&r, "PUT", "/b/_alias/w", Some(r#"{"is_write_index":true}"#)).status,
        200
    );
    let got = members(&r, "w");
    assert_eq!(got["b"]["aliases"]["w"]["is_write_index"], true);
    assert!(got["a"]["aliases"]["w"].get("is_write_index").is_none());
}

// AC-3
#[test]
fn a_delete_keeps_the_alias_until_its_last_member_goes() {
    let r = rig();
    call(&r, "PUT", "/a/_alias/x", None);
    call(&r, "PUT", "/b/_alias/x", None);
    let reply = call(&r, "DELETE", "/a/_alias/x", None);
    assert_eq!((reply.status, reply.text().as_str()), (200, ACK));
    assert_eq!(
        members(&r, "x"),
        serde_json::json!({"b": {"aliases": {"x": {}}}})
    );
    assert_eq!(call(&r, "DELETE", "/a/_alias/x", None).status, 404);
    assert_eq!(call(&r, "DELETE", "/b/_alias/x", None).status, 200);
    assert_eq!(call(&r, "HEAD", "/_alias/x", None).status, 404);
    assert_eq!(
        call(&r, "GET", "/a/_alias", None).text(),
        r#"{"a":{"aliases":{}}}"#
    );
    assert_eq!(call(&r, "DELETE", "/b/_alias/x", None).status, 404);
}

// AC-4
#[test]
fn the_actions_of_one_call_run_in_order() {
    let r = rig();
    let body = r#"{"actions":[
        {"add":{"index":"a","alias":"p"}},
        {"add":{"indices":["b","c"],"aliases":["p","q"]}},
        {"remove":{"index":"c","alias":"q"}}
    ]}"#;
    let reply = call(&r, "POST", "/_aliases", Some(body));
    assert_eq!(
        (reply.status, reply.text().as_str()),
        (200, ACK),
        "{}",
        reply.text()
    );
    let p = members(&r, "p");
    assert_eq!(p.as_object().expect("object").len(), 3);
    assert_eq!(
        members(&r, "q"),
        serde_json::json!({"b": {"aliases": {"q": {}}}})
    );
}

// AC-5
#[test]
fn an_index_reads_its_own_aliases() {
    let r = rig();
    call(&r, "PUT", "/a/_alias/one", None);
    call(&r, "PUT", "/a/_alias/two", None);
    call(&r, "PUT", "/b/_alias/two", None);
    let all = call(&r, "GET", "/a/_alias", None);
    assert_eq!(all.status, 200);
    let got: serde_json::Value = serde_json::from_str(&all.text()).expect("json");
    assert_eq!(
        got,
        serde_json::json!({"a": {"aliases": {"one": {}, "two": {}}}})
    );
    let one = call(&r, "GET", "/a/_alias/two", None);
    let got: serde_json::Value = serde_json::from_str(&one.text()).expect("json");
    assert_eq!(got, serde_json::json!({"a": {"aliases": {"two": {}}}}));
    assert_eq!(call(&r, "GET", "/c/_alias/two", None).status, 404);
    assert_eq!(call(&r, "GET", "/nope/_alias", None).status, 404);
}

// AC-6
#[test]
fn what_is_not_supported_is_refused_and_changes_nothing() {
    let r = rig();
    for body in [
        r#"{"filter":{"term":{"a":1}}}"#,
        r#"{"routing":"1"}"#,
        r#"{"index_routing":"1"}"#,
        r#"{"search_routing":"1"}"#,
        r#"{"is_hidden":true}"#,
    ] {
        let reply = call(&r, "PUT", "/a/_alias/f", Some(body));
        assert_eq!(reply.status, 501, "{body}: {}", reply.text());
    }
    for target in ["/_alias", "/_alias/f*", "/_alias/_all", "/_alias/f,g"] {
        assert_eq!(call(&r, "GET", target, None).status, 501, "{target}");
    }
    assert_eq!(call(&r, "PUT", "/a/_alias/f*", None).status, 501);
    let remove_index = r#"{"actions":[{"remove_index":{"index":"a"}}]}"#;
    assert_eq!(
        call(&r, "POST", "/_aliases", Some(remove_index)).status,
        501
    );
    assert_eq!(call(&r, "HEAD", "/_alias/f", None).status, 404);
    assert_eq!(call(&r, "PUT", "/nope/_alias/f", None).status, 404);
    assert_eq!(call(&r, "PUT", "/a/_alias/b", None).status, 400);
    assert_eq!(call(&r, "PUT", "/a/_alias/_x", None).status, 501);
}

// AC-7
#[test]
fn a_change_costs_requests_that_do_not_grow_with_the_members() {
    let r = rig();
    let cost = |target: &str| {
        let before: Vec<u64> = S3Op::ALL
            .iter()
            .map(|op| r.store.meter().count(*op))
            .collect();
        assert_eq!(call(&r, "PUT", target, None).status, 200);
        let after: Vec<u64> = S3Op::ALL
            .iter()
            .map(|op| r.store.meter().count(*op))
            .collect();
        assert_eq!(r.store.meter().count(S3Op::List), 0);
        after.iter().zip(&before).map(|(a, b)| a - b).sum::<u64>()
    };
    let first = cost("/a/_alias/m");
    let second = cost("/b/_alias/m");
    let third = cost("/c/_alias/m");
    assert!(second <= first + 6, "{first} {second}");
    assert_eq!(second, third);
}
