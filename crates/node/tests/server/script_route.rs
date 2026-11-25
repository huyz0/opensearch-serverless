//! docs/specs/stored-script-routes: store, read and remove scripts by id.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::DocKind;
use oss_mem::MemoryBudget;
use oss_meta::DocSet;
use oss_node::script_route::{ScriptService, routes};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const PAINLESS: &str = r#"{"script":{"lang":"painless","source":"doc['n'].value * 2"}}"#;
const ACK: &str = r#"{"acknowledged":true}"#;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(ScriptService::new(&blobs, "", BUF).expect("service"));
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

fn total(r: &Rig, op: S3Op) -> u64 {
    r.store.meter().count(op)
}

// AC-1
#[test]
fn a_script_is_put_read_replaced_and_deleted() {
    let r = rig();
    let put = call(&r, "PUT", "/_scripts/double", Some(PAINLESS));
    assert_eq!(
        (put.status, put.text().as_str()),
        (200, ACK),
        "{}",
        put.text()
    );
    let got = call(&r, "GET", "/_scripts/double", None);
    assert_eq!(got.status, 200);
    assert_eq!(
        got.text(),
        r#"{"_id":"double","found":true,"script":{"lang":"painless","source":"doc['n'].value * 2"}}"#
    );
    let again = r#"{"script":{"lang":"mustache","source":"{\"q\":1}","options":{"content_type":"application/json"}}}"#;
    assert_eq!(
        call(&r, "POST", "/_scripts/double", Some(again)).status,
        200
    );
    let got = call(&r, "GET", "/_scripts/double", None);
    assert_eq!(
        got.text(),
        r#"{"_id":"double","found":true,"script":{"lang":"mustache","source":"{\"q\":1}","options":{"content_type":"application/json"}}}"#
    );
    // The stored bytes are the Java's form.
    let docs = DocSet::new(
        Arc::clone(&r.store) as Arc<dyn BlobStore>,
        "",
        DocKind::StoredScript,
    )
    .expect("set");
    let mut marker = vec![0u8; BUF];
    let mut body = vec![0u8; BUF];
    let n = docs
        .get("double", &mut marker, &mut body)
        .expect("read")
        .expect("there");
    assert_eq!(
        std::str::from_utf8(&body[..n]).expect("utf8"),
        r#"{"script":{"lang":"mustache","source":"{\"q\":1}","options":{"content_type":"application/json"}}}"#
    );
    let del = call(&r, "DELETE", "/_scripts/double", None);
    assert_eq!((del.status, del.text().as_str()), (200, ACK));
    assert_eq!(call(&r, "GET", "/_scripts/double", None).status, 404);
}

// AC-2
#[test]
fn a_missing_script_is_404() {
    let r = rig();
    let got = call(&r, "GET", "/_scripts/nope", None);
    assert_eq!(got.status, 404);
    assert_eq!(got.text(), r#"{"_id":"nope","found":false}"#);
    assert_eq!(call(&r, "DELETE", "/_scripts/nope", None).status, 404);
}

// AC-3
#[test]
fn a_script_the_java_stored_reads_through_the_route() {
    let r = rig();
    let docs = DocSet::new(
        Arc::clone(&r.store) as Arc<dyn BlobStore>,
        "",
        DocKind::StoredScript,
    )
    .expect("set");
    let mut marker = vec![0u8; BUF];
    docs.put("fromjava", PAINLESS.as_bytes(), false, &mut marker)
        .expect("put");
    let got = call(&r, "GET", "/_scripts/fromjava", None);
    assert_eq!(got.status, 200, "{}", got.text());
    assert!(got.text().contains(r#""found":true"#));
}

// AC-4
#[test]
fn what_is_not_supported_is_refused_and_stores_nothing() {
    let r = rig();
    let refused = |target: &str, body: &str, status: u16| {
        let reply = call(&r, "PUT", target, Some(body));
        assert_eq!(reply.status, status, "{target} {body}: {}", reply.text());
    };
    refused("/_scripts/x/score", PAINLESS, 501);
    refused("/_scripts/_x", PAINLESS, 501);
    refused(
        "/_scripts/x",
        r#"{"script":{"lang":"ruby","source":"1"}}"#,
        400,
    );
    refused(
        "/_scripts/x",
        r#"{"script":{"lang":"painless","source":""}}"#,
        400,
    );
    refused("/_scripts/x", r#"{"script":{"lang":"painless"}}"#, 400);
    refused("/_scripts/x", r#"{"script":{"source":"1"}}"#, 400);
    refused("/_scripts/x", r#"{"nope":1}"#, 400);
    refused("/_scripts/x", "[1]", 400);
    refused(
        "/_scripts/x",
        r#"{"script":{"lang":"painless","source":"1","options":{"bad":"x"}}}"#,
        400,
    );
    let big = format!(
        r#"{{"script":{{"lang":"painless","source":"{}"}}}}"#,
        "a".repeat(65_536)
    );
    refused("/_scripts/x", &big, 400);
    assert_eq!(call(&r, "GET", "/_scripts/x", None).status, 404);
}

// AC-5
#[test]
fn the_requests_are_the_document_sets() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_scripts/a", Some(PAINLESS)).status, 200);
    assert_eq!((total(&r, S3Op::Get), total(&r, S3Op::Put)), (1, 2));
    assert_eq!(call(&r, "GET", "/_scripts/a", None).status, 200);
    assert_eq!(total(&r, S3Op::Get), 3);
    assert_eq!(total(&r, S3Op::List), 0);
}
