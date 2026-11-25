//! docs/specs/index-metadata-block-aliases-delete: alias changes and the
//! deletion of an index with `index.blocks.metadata` are refused.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator};
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore, PitClock};
use oss_node::alias_route::{AliasService, routes as alias_routes};
use oss_node::index_delete::{DeleteService, NoSnapshots, routes as delete_routes};
use oss_node::index_route::{IndexService, routes as index_routes};
use oss_node::settings_route::{SettingsService, routes as settings_routes};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const BLOCK: &str = "FORBIDDEN/9/index metadata (api)";

fn rig() -> Server {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let dynstore: Arc<dyn BlobStore> = store;
    let descriptors =
        || DescriptorStore::new(Arc::clone(&dynstore), "", DeleteMode::Conditional).expect("names");
    let clock: PitClock = Arc::new(|| 1_234_567);
    let aliases = Arc::new(AliasService::new(
        descriptors(),
        Arc::clone(&clock),
        BUF,
        30_000,
    ));
    let indices = Arc::new(IndexService::new(descriptors(), Arc::clone(&clock), BUF));
    let settings = Arc::new(SettingsService::new(descriptors(), BUF, false));
    let deletes = Arc::new(
        DeleteService::new(
            &dynstore,
            "",
            DeleteMode::Conditional,
            clock,
            Arc::new(NoSnapshots),
            BUF,
            2,
        )
        .expect("service"),
    );
    let router = index_routes(Router::new(), indices).expect("routes");
    let router = alias_routes(router, aliases).expect("routes");
    let router = settings_routes(router, &settings).expect("routes");
    let router = delete_routes(router, deletes).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    for index in ["a", "b"] {
        assert_eq!(call(&server, "PUT", &format!("/{index}"), None).status, 200);
    }
    server
}

fn call(s: &Server, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(s.local_addr(), method, target, body.map(str::as_bytes))
}

fn block(s: &Server, index: &str, on: bool) {
    let body = format!(r#"{{"index.blocks.metadata":{on}}}"#);
    let reply = call(s, "PUT", &format!("/{index}/_settings"), Some(&body));
    assert_eq!(reply.status, 200, "{}", reply.text());
}

fn assert_blocked(reply: &support::Reply, index: &str) {
    assert_eq!(reply.status, 403, "{}", reply.text());
    let text = reply.text();
    assert!(
        text.contains("cluster_block_exception")
            && text.contains(&format!("index [{index}] blocked by: [{BLOCK}];")),
        "{text}"
    );
}

/// The names of the indices behind alias `x`, or `None` when it is absent.
fn members(s: &Server) -> Option<String> {
    let reply = call(s, "GET", "/_alias/x", None);
    (reply.status == 200).then(|| reply.text())
}

// AC-1, AC-5
#[test]
fn an_alias_add_naming_a_blocked_index_is_refused() {
    let s = rig();
    block(&s, "a", true);
    assert_blocked(&call(&s, "PUT", "/a/_alias/x", None), "a");
    assert_eq!(members(&s), None);
    assert_eq!(call(&s, "PUT", "/b/_alias/x", None).status, 200);
    block(&s, "a", false);
    assert_eq!(call(&s, "PUT", "/a/_alias/x", None).status, 200);
}

// AC-2
#[test]
fn an_alias_remove_naming_a_blocked_index_is_refused() {
    let s = rig();
    assert_eq!(call(&s, "PUT", "/a/_alias/x", None).status, 200);
    block(&s, "a", true);
    assert_blocked(&call(&s, "DELETE", "/a/_alias/x", None), "a");
    assert!(members(&s).is_some_and(|m| m.contains("\"a\"")));
}

// AC-3
#[test]
fn a_blocked_index_among_several_actions_stops_them_all() {
    let s = rig();
    block(&s, "b", true);
    let body =
        r#"{"actions":[{"add":{"index":"a","alias":"x"}},{"add":{"index":"b","alias":"x"}}]}"#;
    assert_blocked(&call(&s, "POST", "/_aliases", Some(body)), "b");
    assert_eq!(members(&s), None, "no action was applied");
}

// AC-4
#[test]
fn a_blocked_index_is_not_deleted_until_the_block_is_lifted() {
    let s = rig();
    block(&s, "a", true);
    assert_blocked(&call(&s, "DELETE", "/a", None), "a");
    assert_eq!(call(&s, "HEAD", "/a", None).status, 200);
    block(&s, "a", false);
    assert_eq!(call(&s, "DELETE", "/a", None).status, 200);
    assert_eq!(call(&s, "HEAD", "/a", None).status, 404);
}

// AC-5
#[test]
fn a_missing_index_and_an_unblocked_one_answer_as_before() {
    let s = rig();
    assert_eq!(call(&s, "DELETE", "/nope", None).status, 404);
    assert_eq!(call(&s, "PUT", "/nope/_alias/x", None).status, 404);
    assert_eq!(call(&s, "DELETE", "/b", None).status, 200);
}
