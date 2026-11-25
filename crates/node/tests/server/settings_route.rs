//! docs/specs/settings-update-route: change the settings of one index.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator};
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore, PitClock, Target};
use oss_node::alias_route::{AliasService, routes as alias_routes};
use oss_node::index_route::{IndexService, routes as index_routes};
use oss_node::settings_route::{SettingsService, routes};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const ACK: &str = r#"{"acknowledged":true}"#;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn descriptors(store: &Arc<Emulator>) -> DescriptorStore {
    DescriptorStore::new(
        Arc::clone(store) as Arc<dyn BlobStore>,
        "",
        DeleteMode::Conditional,
    )
    .expect("names")
}

fn rig() -> Rig {
    rig_capped(1_000)
}

fn rig_capped(cap: usize) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let clock: PitClock = Arc::new(|| 1_234_567);
    let aliases = Arc::new(AliasService::new(
        descriptors(&store),
        Arc::clone(&clock),
        BUF,
        30_000,
    ));
    let indices = Arc::new(IndexService::new(descriptors(&store), clock, BUF));
    let settings =
        Arc::new(SettingsService::new(descriptors(&store), BUF, false).with_pattern_cap(cap));
    let router = index_routes(Router::new(), indices).expect("routes");
    let router = alias_routes(router, aliases).expect("routes");
    let router = routes(router, &settings).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    let r = Rig { server, store };
    assert_eq!(call(&r, "PUT", "/a", None).status, 200);
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

/// The stored settings text and version of index `a`.
fn stored(r: &Rig) -> (String, i64) {
    let mut buf = vec![0u8; BUF];
    let held = descriptors(&r.store).resolve("a", &mut buf).expect("reads");
    match held.target {
        Target::Index(rec) => (
            String::from_utf8(rec.settings.to_vec()).expect("utf8"),
            rec.settings_version,
        ),
        _ => panic!("not an index"),
    }
}

// AC-1
#[test]
fn an_update_stores_the_value_and_bumps_the_version() {
    let r = rig();
    let (_, before) = stored(&r);
    let reply = call(
        &r,
        "PUT",
        "/a/_settings",
        Some(r#"{"index":{"refresh_interval":"5s","priority":3}}"#),
    );
    assert_eq!((reply.status, reply.text().as_str()), (200, ACK));
    let (text, after) = stored(&r);
    assert_eq!(after, before + 1);
    assert!(text.contains(r#""index.refresh_interval":"5s""#), "{text}");
    assert!(text.contains(r#""index.priority":3"#), "{text}");
}

// AC-2
#[test]
fn a_refused_update_changes_nothing() {
    let r = rig();
    let (text, version) = stored(&r);
    for (body, status) in [
        (r#"{"index.no_such":1}"#, 400),
        (r#"{"index.number_of_shards":3}"#, 501),
        (r#"{"index.refresh_interval":"soon"}"#, 400),
        (r#"{"index.codec":"best_compression"}"#, 501),
        (r#"{"index.blocks.read":true}"#, 501),
        (r#"{"index.blocks.write":"yes"}"#, 400),
        (r#"{"index.max_result_window":10001}"#, 400),
        (r#"{"index.max_result_window":0}"#, 400),
        (r#"{"index.max_result_window":"x"}"#, 400),
        ("", 400),
    ] {
        let reply = call(&r, "PUT", "/a/_settings", Some(body));
        assert_eq!(reply.status, status, "{body}: {}", reply.text());
    }
    assert_eq!(stored(&r), (text, version));
}

// AC-3
#[test]
fn an_update_that_changes_nothing_writes_nothing() {
    let r = rig();
    let body = r#"{"index.refresh_interval":"5s"}"#;
    assert_eq!(call(&r, "PUT", "/a/_settings", Some(body)).status, 200);
    let once = stored(&r);
    assert_eq!(call(&r, "PUT", "/a/_settings", Some(body)).status, 200);
    assert_eq!(stored(&r), once);
}

// AC-4
#[test]
fn a_missing_index_is_404() {
    let r = rig();
    let reply = call(
        &r,
        "PUT",
        "/nope/_settings",
        Some(r#"{"index.priority":1}"#),
    );
    assert_eq!(reply.status, 404, "{}", reply.text());
    assert!(reply.text().contains("index_not_found_exception"));
}

// AC-6
#[test]
fn a_null_removes_and_preserve_existing_keeps() {
    let r = rig();
    let set = r#"{"index.refresh_interval":"5s"}"#;
    assert_eq!(call(&r, "PUT", "/a/_settings", Some(set)).status, 200);
    let keep = r#"{"index.refresh_interval":"9s"}"#;
    let reply = call(&r, "PUT", "/a/_settings?preserve_existing=true", Some(keep));
    assert_eq!(reply.status, 200);
    assert!(stored(&r).0.contains(r#""5s""#));
    let reset = r#"{"index.refresh_interval":null}"#;
    assert_eq!(call(&r, "PUT", "/a/_settings", Some(reset)).status, 200);
    assert!(!stored(&r).0.contains("refresh_interval"));
}

// docs/specs/index-blocks AC-3, AC-4
#[test]
fn a_write_block_set_through_the_route_is_what_a_bulk_reads_and_false_lifts_it() {
    use oss_node::descriptor_routes::DescriptorRoutes;
    use oss_node::writer::Routes;
    use oss_writer::bulk::WriteBlock;
    let r = rig();
    let routes = DescriptorRoutes::new(descriptors(&r.store), BUF);
    let block = || routes.index("a").expect("found").write_block;
    assert_eq!(block(), WriteBlock::None);
    let on = r#"{"index.blocks.write":true}"#;
    assert_eq!(call(&r, "PUT", "/a/_settings", Some(on)).status, 200);
    assert_eq!(block(), WriteBlock::Full);
    let delete = r#"{"index.blocks.write":null,"index.blocks.read_only_allow_delete":"true"}"#;
    assert_eq!(call(&r, "PUT", "/a/_settings", Some(delete)).status, 200);
    assert_eq!(block(), WriteBlock::AllowDelete);
    let off = r#"{"index.blocks.read_only_allow_delete":false}"#;
    assert_eq!(call(&r, "PUT", "/a/_settings", Some(off)).status, 200);
    assert_eq!(block(), WriteBlock::None);
}

// docs/specs/index-max-result-window AC-5, AC-6
#[test]
fn a_result_window_is_stored_where_the_reader_reads_it() {
    let r = rig();
    let reply = call(
        &r,
        "PUT",
        "/a/_settings",
        Some(r#"{"index.max_result_window":50}"#),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    let settings = oss_format::ClusterSettings::parse(stored(&r).0.as_bytes()).expect("parses");
    assert_eq!(
        settings.get("index.max_result_window"),
        Some(&oss_format::Setting::Value("50".into()))
    );
    // The node's ceiling here is the reader's.
    let at = format!(
        r#"{{"index.max_result_window":{}}}"#,
        oss_reader::search::MAX_RESULT_WINDOW
    );
    assert_eq!(call(&r, "PUT", "/a/_settings", Some(&at)).status, 200);
    let over = format!(
        r#"{{"index.max_result_window":{}}}"#,
        oss_reader::search::MAX_RESULT_WINDOW + 1
    );
    assert_eq!(call(&r, "PUT", "/a/_settings", Some(&over)).status, 400);
}

/// The settings version of index `name`, none when it is no index.
fn version(r: &Rig, name: &str) -> Option<i64> {
    let mut buf = vec![0u8; BUF];
    match descriptors(&r.store)
        .resolve(name, &mut buf)
        .expect("reads")
        .target
    {
        Target::Index(rec) => Some(rec.settings_version),
        _ => None,
    }
}

fn with_indices(names: &[&str]) -> Rig {
    let r = rig();
    for name in names {
        assert_eq!(call(&r, "PUT", &format!("/{name}"), None).status, 200);
    }
    r
}

const SET: &str = r#"{"index.priority":2}"#;

// docs/specs/settings-update-many AC-1
#[test]
fn a_list_of_names_updates_each() {
    let r = with_indices(&["b", "c"]);
    let before = [version(&r, "a"), version(&r, "b"), version(&r, "c")];
    let reply = call(&r, "PUT", "/a,b/_settings", Some(SET));
    assert_eq!((reply.status, reply.text().as_str()), (200, ACK));
    assert_eq!(version(&r, "a"), before[0].map(|v| v + 1));
    assert_eq!(version(&r, "b"), before[1].map(|v| v + 1));
    assert_eq!(version(&r, "c"), before[2]);
}

// AC-2
#[test]
fn a_prefix_pattern_updates_what_it_matches_and_none_is_acknowledged() {
    let r = with_indices(&["logs-1", "logs-2", "other"]);
    let reply = call(&r, "PUT", "/logs-*/_settings", Some(SET));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(version(&r, "logs-1"), Some(2));
    assert_eq!(version(&r, "logs-2"), Some(2));
    assert_eq!(version(&r, "other"), Some(1));
    let none = call(&r, "PUT", "/nothing-*/_settings", Some(SET));
    assert_eq!((none.status, none.text().as_str()), (200, ACK));
}

// AC-3
#[test]
fn an_alias_stands_for_its_members_and_a_repeat_updates_once() {
    let r = with_indices(&["b"]);
    assert_eq!(call(&r, "PUT", "/a/_alias/both", None).status, 200);
    assert_eq!(call(&r, "PUT", "/b/_alias/both", None).status, 200);
    let reply = call(&r, "PUT", "/both,a,a*/_settings", Some(SET));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(version(&r, "a"), Some(2));
    assert_eq!(version(&r, "b"), Some(2));
}

// AC-4
#[test]
fn every_index_is_named_by_all_a_star_or_no_name() {
    for target in ["/_settings", "/_all/_settings", "/*/_settings"] {
        let r = with_indices(&["b"]);
        let reply = call(&r, "PUT", target, Some(SET));
        assert_eq!(reply.status, 200, "{target}: {}", reply.text());
        assert_eq!((version(&r, "a"), version(&r, "b")), (Some(2), Some(2)));
    }
}

// AC-5, AC-6
#[test]
fn a_refusal_before_the_first_write_changes_nothing() {
    let r = with_indices(&["b"]);
    let bad = call(&r, "PUT", "/a,b/_settings", Some(r#"{"index.nope":1}"#));
    assert_eq!(bad.status, 400);
    let missing = call(&r, "PUT", "/a,nope,b/_settings", Some(SET));
    assert_eq!(missing.status, 404, "{}", missing.text());
    let inner = call(&r, "PUT", "/a,l*g/_settings", Some(SET));
    assert_eq!(inner.status, 501, "{}", inner.text());
    assert!(inner.text().contains("l*g"));
    assert_eq!((version(&r, "a"), version(&r, "b")), (Some(1), Some(1)));
}

// docs/specs/settings-update-many AC-4
#[test]
fn a_pattern_past_the_cap_is_refused_and_changes_nothing() {
    let r = rig_capped(2);
    for name in ["b", "c"] {
        assert_eq!(call(&r, "PUT", &format!("/{name}"), None).status, 200);
    }
    let reply = call(&r, "PUT", "/_settings", Some(SET));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(reply.text().contains("too_many_indices"));
    assert_eq!(version(&r, "a"), Some(1));
}
