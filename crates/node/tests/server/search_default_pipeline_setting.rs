//! docs/specs/search-default-pipeline AC-1, AC-2: `index.search.default_pipeline`
//! on a create and on a settings update.

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
const KEY: &str = "index.search.default_pipeline";

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

fn rig(search_pipelines: bool) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let clock: PitClock = Arc::new(|| 1_234_567);
    let indices = Arc::new(
        IndexService::new(descriptors(&store), clock, BUF).with_search_pipelines(search_pipelines),
    );
    let settings = Arc::new(
        SettingsService::new(descriptors(&store), BUF, false)
            .with_search_pipelines(search_pipelines),
    );
    let router = index_routes(Router::new(), indices).expect("routes");
    let router = routes(router, &settings).expect("routes");
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

fn stored(r: &Rig, index: &str) -> String {
    let mut buf = vec![0u8; BUF];
    let held = descriptors(&r.store)
        .resolve(index, &mut buf)
        .expect("reads");
    match held.target {
        Target::Index(rec) => String::from_utf8(rec.settings.to_vec()).expect("utf8"),
        _ => panic!("not an index"),
    }
}

fn create(r: &Rig, name: &str, value: &str) -> support::Reply {
    let body = format!(r#"{{"settings":{{"{KEY}":{value}}}}}"#);
    call(r, "PUT", &format!("/{name}"), Some(&body))
}

// AC-1
#[test]
fn a_create_takes_the_key_when_search_pipelines_are_on() {
    let r = rig(true);
    assert_eq!(create(&r, "a", r#""p""#).status, 200);
    assert!(stored(&r, "a").contains(&format!(r#""{KEY}":"p""#)));
    assert_eq!(create(&r, "b", r#""_none""#).status, 200);
    assert_eq!(create(&r, "c", r#""""#).status, 400);
}

// AC-1
#[test]
fn a_create_refuses_the_key_when_search_pipelines_are_off() {
    let r = rig(false);
    let reply = create(&r, "a", r#""p""#);
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(
        reply.text().contains("search pipelines"),
        "{}",
        reply.text()
    );
}

// AC-2
#[test]
fn an_update_sets_replaces_and_removes_the_key_when_on() {
    let r = rig(true);
    assert_eq!(call(&r, "PUT", "/a", None).status, 200);
    let put = |body: &str| call(&r, "PUT", "/a/_settings", Some(body));
    assert_eq!(put(&format!(r#"{{"{KEY}":"p"}}"#)).status, 200);
    assert!(stored(&r, "a").contains(&format!(r#""{KEY}":"p""#)));
    assert_eq!(put(&format!(r#"{{"{KEY}":"q"}}"#)).status, 200);
    assert!(stored(&r, "a").contains(&format!(r#""{KEY}":"q""#)));
    assert_eq!(put(&format!(r#"{{"{KEY}":null}}"#)).status, 200);
    assert!(!stored(&r, "a").contains(KEY), "{}", stored(&r, "a"));
    assert_eq!(put(&format!(r#"{{"{KEY}":""}}"#)).status, 400);
}

// AC-2
#[test]
fn an_update_refuses_the_key_when_off() {
    let r = rig(false);
    assert_eq!(call(&r, "PUT", "/a", None).status, 200);
    let reply = call(
        &r,
        "PUT",
        "/a/_settings",
        Some(&format!(r#"{{"{KEY}":"p"}}"#)),
    );
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(
        reply.text().contains("?search_pipeline="),
        "{}",
        reply.text()
    );
    assert!(!stored(&r, "a").contains(KEY));
}
