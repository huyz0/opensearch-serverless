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
use oss_testkit::budget;

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

// AC-3: the check precedes every action, wherever the blocked index sits.
#[test]
fn the_check_runs_before_the_first_action_wherever_the_blocked_index_is() {
    for blocked in ["a", "b", "c"] {
        let s = rig();
        assert_eq!(call(&s, "PUT", "/c", None).status, 200);
        block(&s, blocked, true);
        let body = r#"{"actions":[{"add":{"index":"a","alias":"x"}},{"add":{"index":"b","alias":"x"}},{"add":{"index":"c","alias":"x"}}]}"#;
        assert_blocked(&call(&s, "POST", "/_aliases", Some(body)), blocked);
        assert_eq!(members(&s), None, "{blocked}: no action was applied");
    }
}

// AC-2: removing an alias that does not exist from a blocked index is the block's 403.
#[test]
fn a_blocked_index_answers_the_block_before_the_alias_lookup() {
    let s = rig();
    block(&s, "a", true);
    assert_blocked(&call(&s, "DELETE", "/a/_alias/nope", None), "a");
}

// The key is read as the descriptor stores it: nested, or a string.
#[test]
fn the_block_is_read_nested_and_as_a_string() {
    let s = rig();
    for body in [
        r#"{"index":{"blocks":{"metadata":true}}}"#,
        r#"{"index.blocks.metadata":"true"}"#,
    ] {
        let reply = call(&s, "PUT", "/a/_settings", Some(body));
        assert_eq!(reply.status, 200, "{body}: {}", reply.text());
        assert_blocked(&call(&s, "PUT", "/a/_alias/x", None), "a");
        block(&s, "a", false);
        assert_eq!(call(&s, "PUT", "/a/_alias/y", None).status, 200);
        assert_eq!(call(&s, "DELETE", "/a/_alias/y", None).status, 200);
    }
}

/// The GETs the store has served.
fn gets(s: &crate::rollover_route::Rig) -> u64 {
    s.store.meter().count(oss_blob::S3Op::Get)
}

// AC-6: a rollover moves the alias off the old index, so the block stops it.
#[test]
fn a_rollover_of_a_blocked_index_is_refused_and_changes_nothing() {
    use crate::rollover_route::{call, rig};
    let r = rig();
    assert_eq!(call(&r, "PUT", "/logs-000001", None).status, 200);
    assert_eq!(
        call(
            &r,
            "PUT",
            "/logs-000001/_alias/w",
            Some(r#"{"is_write_index":true}"#)
        )
        .status,
        200
    );
    let body = r#"{"index.blocks.metadata":true}"#;
    assert_eq!(
        call(&r, "PUT", "/logs-000001/_settings", Some(body)).status,
        200
    );
    let refused = call(&r, "POST", "/w/_rollover", None);
    assert_eq!(refused.status, 403, "{}", refused.text());
    assert!(refused.text().contains(BLOCK), "{}", refused.text());
    assert_eq!(
        call(&r, "HEAD", "/logs-000002", None).status,
        404,
        "no new index"
    );
    // A dry run changes nothing, so it is not refused.
    assert_eq!(
        call(&r, "POST", "/w/_rollover?dry_run=true", None).status,
        200
    );
}

// AC-7: a data stream with a blocked backing index is not deleted, and stays whole.
#[test]
fn a_data_stream_with_a_blocked_backing_index_is_not_deleted() {
    use crate::rollover_route::{call, indices_of, rig};
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    assert_eq!(call(&r, "POST", "/logs-1/_rollover", None).status, 200);
    let backing = indices_of(&r, "logs-1");
    assert_eq!(backing.len(), 2);
    let body = r#"{"index.blocks.metadata":true}"#;
    let set = call(&r, "PUT", &format!("/{}/_settings", backing[1]), Some(body));
    assert_eq!(set.status, 200, "{}", set.text());
    let refused = call(&r, "DELETE", "/_data_stream/logs-1", None);
    assert_eq!(refused.status, 403, "{}", refused.text());
    assert_eq!(indices_of(&r, "logs-1"), backing, "the stream is whole");
}

// The alias route reads one descriptor for each distinct index it names.
#[test]
fn an_alias_request_reads_one_descriptor_for_each_distinct_index() {
    use crate::rollover_route::{call, rig};
    let r = rig();
    for index in ["a", "b"] {
        assert_eq!(call(&r, "PUT", &format!("/{index}"), None).status, 200);
    }
    let body = r#"{"index.blocks.metadata":true}"#;
    assert_eq!(call(&r, "PUT", "/a/_settings", Some(body)).status, 200);
    let refused = |actions: &str| {
        let before = gets(&r);
        let reply = call(&r, "POST", "/_aliases", Some(actions));
        assert_eq!(reply.status, 403, "{}", reply.text());
        gets(&r) - before
    };
    let add =
        |index: &str, alias: &str| format!(r#"{{"add":{{"index":"{index}","alias":"{alias}"}}}}"#);
    let same = format!(
        r#"{{"actions":[{},{},{}]}}"#,
        add("a", "x"),
        add("a", "y"),
        add("a", "z")
    );
    let two = format!(r#"{{"actions":[{},{}]}}"#, add("b", "x"), add("a", "x"));
    budget::check(
        "alias.metadata_block.requests",
        refused(&same),
        "three actions naming one blocked index",
    );
    assert_eq!(refused(&two), 2, "one for each distinct index");
}
