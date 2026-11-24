//! `_index_template` and `_component_template` over a node's document sets
//! (docs/specs/template-routes/SPEC.md).

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_node::template_route::{TemplateService, routes};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const LOGS: &str = r#"{"index_patterns":["logs-*"],"priority":5}"#;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(TemplateService::new(&blobs, "", BUF).expect("service"));
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
fn a_template_is_stored_read_listed_and_removed() {
    let r = rig();
    let put = call(&r, "PUT", "/_index_template/logs", Some(LOGS));
    assert_eq!(
        (put.status, text(&put)),
        (200, r#"{"acknowledged":true}"#.to_owned())
    );
    assert_eq!(call(&r, "HEAD", "/_index_template/logs", None).status, 200);
    assert_eq!(call(&r, "HEAD", "/_index_template/none", None).status, 404);
    let one = call(&r, "GET", "/_index_template/logs", None);
    assert_eq!(
        text(&one),
        format!(r#"{{"index_templates":[{{"name":"logs","index_template":{LOGS}}}]}}"#)
    );
    assert_eq!(
        call(&r, "POST", "/_index_template/metrics", Some(LOGS)).status,
        200
    );
    let all = text(&call(&r, "GET", "/_index_template", None));
    assert!(
        all.contains(r#""name":"logs""#) && all.contains(r#""name":"metrics""#),
        "{all}"
    );
    let prefix = text(&call(&r, "GET", "/_index_template/lo*", None));
    assert!(
        prefix.contains("logs") && !prefix.contains("metrics"),
        "{prefix}"
    );
    assert_eq!(
        call(&r, "DELETE", "/_index_template/logs", None).status,
        200
    );
    let gone = call(&r, "DELETE", "/_index_template/logs", None);
    assert_eq!(gone.status, 404);
    assert!(text(&gone).contains("template_missing"));
    assert_eq!(call(&r, "GET", "/_index_template/logs", None).status, 404);
}

#[test]
fn the_two_sets_are_separate() {
    let r = rig();
    assert_eq!(
        call(
            &r,
            "PUT",
            "/_component_template/base",
            Some(r#"{"template":{}}"#)
        )
        .status,
        200
    );
    assert_eq!(call(&r, "GET", "/_index_template/base", None).status, 404);
    let got = text(&call(&r, "GET", "/_component_template", None));
    assert_eq!(
        got,
        r#"{"component_templates":[{"name":"base","component_template":{"template":{}}}]}"#
    );
    let refused = call(&r, "PUT", "/_component_template/bad", Some(LOGS));
    assert_eq!(refused.status, 400);
}

#[test]
fn what_cannot_be_honoured_is_refused_and_stores_nothing() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_index_template/x", None).status, 400);
    assert_eq!(
        call(&r, "PUT", "/_index_template/x", Some("[]")).status,
        400
    );
    assert_eq!(
        call(&r, "PUT", "/_index_template/_simulate", Some(LOGS)).status,
        501
    );
    assert_eq!(
        call(&r, "PUT", "/_index_template/a*", Some(LOGS)).status,
        501
    );
    let aliased = r#"{"index_patterns":["a"],"template":{"aliases":{"x":{}}}}"#;
    assert_eq!(
        call(&r, "PUT", "/_index_template/x", Some(aliased)).status,
        501
    );
    assert_eq!(call(&r, "GET", "/_index_template/x", None).status, 404);
}

#[test]
fn create_true_refuses_a_name_that_exists() {
    let r = rig();
    assert_eq!(
        call(&r, "PUT", "/_index_template/logs", Some(LOGS)).status,
        200
    );
    let again = call(&r, "PUT", "/_index_template/logs?create=true", Some(LOGS));
    assert_eq!(again.status, 400);
    assert!(text(&again).contains("already exists"), "{}", text(&again));
    assert_eq!(
        call(&r, "PUT", "/_index_template/logs", Some(LOGS)).status,
        200
    );
}

#[test]
fn a_read_of_an_unchanged_set_is_one_get_and_nothing_is_listed() {
    let r = rig();
    assert_eq!(
        call(&r, "PUT", "/_index_template/logs", Some(LOGS)).status,
        200
    );
    assert_eq!(call(&r, "GET", "/_index_template", None).status, 200);
    let total = |r: &Rig| -> u64 { S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum() };
    let before = total(&r);
    assert_eq!(call(&r, "GET", "/_index_template", None).status, 200);
    assert_eq!(total(&r) - before, 1, "one marker GET");
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}
