//! docs/specs/settings-update-route: change the settings of one index.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator};
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore, PitClock, Target};
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
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let clock: PitClock = Arc::new(|| 1_234_567);
    let indices = Arc::new(IndexService::new(descriptors(&store), clock, BUF));
    let settings = Arc::new(SettingsService::new(descriptors(&store), BUF, false));
    let router = index_routes(Router::new(), indices).expect("routes");
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
        (r#"{"index.blocks.write":true}"#, 501),
        (r#"{"index.max_result_window":50}"#, 501),
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
