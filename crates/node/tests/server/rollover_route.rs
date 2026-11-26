//! docs/specs/rollover-route: a new index behind an alias or a data stream.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::DocKind;
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore, DocSet, PitClock};
use oss_node::alias_route::{AliasService, routes as alias_routes};
use oss_node::data_stream_route::{DataStreamService, routes as stream_routes};
use oss_node::index_delete::{DeleteService, NoSnapshots, routes as delete_routes};
use oss_node::index_route::{IndexService, routes as index_routes};
use oss_node::index_templates::TemplateSets;
use oss_node::rollover_route::{DocCount, RolloverService, routes};
use oss_node::settings_route::{SettingsService, routes as settings_routes};
use oss_rest::{Router, Server};
use oss_testkit::budget;

pub(crate) const BUF: usize = 1 << 16;
const TTL: i64 = 30_000;
const NOW: u64 = 1_000_000;
const DAY: u64 = 86_400_000;
const LOGS: &str = r#"{"index_patterns":["logs-*"],"data_stream":{},"template":{"mappings":{"properties":{"msg":{"type":"text"}}}}}"#;

pub(crate) struct Rig {
    pub(crate) server: Server,
    pub(crate) store: Arc<Emulator>,
    pub(crate) clock: Arc<AtomicU64>,
}

pub(crate) fn rig() -> Rig {
    rig_with(None)
}

fn rig_with(join: Option<oss_node::index_route::BucketJoin>) -> Rig {
    rig_full(join, None)
}

/// A count of documents set by the test, remembering what it was asked.
pub(crate) struct Counted {
    pub(crate) docs: AtomicU64,
    pub(crate) asked: std::sync::Mutex<Vec<(String, u32)>>,
}

impl DocCount for Counted {
    fn count(
        &self,
        index: &str,
        shards: u32,
    ) -> Result<u64, oss_coordinator::route::resolve::Refusal> {
        self.asked
            .lock()
            .expect("lock")
            .push((index.to_owned(), shards));
        Ok(self.docs.load(Ordering::Relaxed))
    }
}

pub(crate) fn rig_full(
    join: Option<oss_node::index_route::BucketJoin>,
    counter: Option<Arc<Counted>>,
) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = || Arc::clone(&store) as Arc<dyn BlobStore>;
    let mut marker = vec![0u8; BUF];
    DocSet::new(blobs(), "", DocKind::IndexTemplate)
        .expect("set")
        .put("logs", LOGS.as_bytes(), false, &mut marker)
        .expect("seeded");
    let now = Arc::new(AtomicU64::new(NOW));
    let tick = Arc::clone(&now);
    let clock: PitClock = Arc::new(move || tick.load(Ordering::Relaxed));
    let descriptors = || DescriptorStore::new(blobs(), "", DeleteMode::Conditional).expect("names");
    let sets = TemplateSets::new(
        DocSet::new(blobs(), "", DocKind::IndexTemplate).expect("set"),
        DocSet::new(blobs(), "", DocKind::ComponentTemplate).expect("set"),
    );
    let mut service =
        IndexService::new(descriptors(), Arc::clone(&clock), BUF).with_templates(sets);
    if let Some(join) = join {
        service = service.with_bucket_join(join, Arc::new(|_, _| Ok(())));
    }
    let indices = Arc::new(service);
    let deletes = Arc::new(
        DeleteService::new(
            &blobs(),
            "",
            DeleteMode::Conditional,
            Arc::clone(&clock),
            Arc::new(NoSnapshots),
            BUF,
            2,
        )
        .expect("deletes"),
    );
    let streams = Arc::new(DataStreamService::new(
        Arc::clone(&indices),
        Arc::clone(&deletes),
        descriptors(),
        Arc::clone(&clock),
        BUF,
        TTL,
    ));
    let mut rollover = RolloverService::new(
        Arc::clone(&indices),
        Arc::clone(&deletes),
        descriptors(),
        Arc::clone(&clock),
        BUF,
        TTL,
    );
    if let Some(counter) = counter {
        rollover = rollover.with_counter(counter);
    }
    let rollover = Arc::new(rollover);
    let aliases = Arc::new(AliasService::new(
        descriptors(),
        Arc::clone(&clock),
        BUF,
        TTL,
    ));
    let router = index_routes(Router::new(), indices).expect("routes");
    let router = alias_routes(router, aliases).expect("routes");
    let router = stream_routes(router, streams).expect("routes");
    let router = routes(router, rollover).expect("routes");
    let router = delete_routes(router, deletes).expect("routes");
    let settings = Arc::new(SettingsService::new(descriptors(), BUF, false));
    let router = settings_routes(router, &settings).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        store,
        clock: now,
    }
}

pub(crate) fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

pub(crate) fn json(reply: &support::Reply) -> serde_json::Value {
    serde_json::from_str(&reply.text()).unwrap_or_else(|e| panic!("{e}: {}", reply.text()))
}

fn requests(r: &Rig) -> u64 {
    [S3Op::Get, S3Op::Put, S3Op::Delete, S3Op::Head, S3Op::List]
        .iter()
        .map(|op| r.store.meter().count(*op))
        .sum()
}

fn kind(reply: &support::Reply) -> String {
    json(reply)["error"]["type"]
        .as_str()
        .unwrap_or("")
        .to_owned()
}

pub(crate) fn indices_of(r: &Rig, stream: &str) -> Vec<String> {
    let got = json(&call(r, "GET", &format!("/_data_stream/{stream}"), None));
    got["data_streams"][0]["indices"]
        .as_array()
        .expect("indices")
        .iter()
        .map(|i| i["index_name"].as_str().expect("name").to_owned())
        .collect()
}

// AC-1
#[test]
fn a_stream_rolls_over_to_the_next_generation() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    let rolled = call(&r, "POST", "/logs-1/_rollover", None);
    assert_eq!(rolled.status, 200, "{}", rolled.text());
    assert_eq!(
        json(&rolled),
        serde_json::json!({
            "acknowledged": true, "shards_acknowledged": true,
            "old_index": ".ds-logs-1-000001", "new_index": ".ds-logs-1-000002",
            "rolled_over": true, "dry_run": false, "conditions": {}
        })
    );
    assert_eq!(
        indices_of(&r, "logs-1"),
        [".ds-logs-1-000001", ".ds-logs-1-000002"]
    );
    let got = json(&call(&r, "GET", "/_data_stream/logs-1", None));
    assert_eq!(got["data_streams"][0]["generation"], 2);
    assert_eq!(call(&r, "POST", "/logs-1/_rollover", None).status, 200);
    assert_eq!(indices_of(&r, "logs-1").len(), 3);
    // The write index is the newest; an older one deletes.
    assert_eq!(call(&r, "DELETE", "/.ds-logs-1-000003", None).status, 400);
    assert_eq!(call(&r, "DELETE", "/.ds-logs-1-000001", None).status, 200);
}

// AC-2
#[test]
fn conditions_decide_whether_it_rolls() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    let body = r#"{"conditions":{"max_age":"1d"}}"#;
    let young = call(&r, "POST", "/logs-1/_rollover", Some(body));
    assert_eq!(
        json(&young),
        serde_json::json!({
            "acknowledged": false, "shards_acknowledged": false,
            "old_index": ".ds-logs-1-000001", "new_index": ".ds-logs-1-000002",
            "rolled_over": false, "dry_run": false, "conditions": {"[max_age: 1d]": false}
        })
    );
    assert_eq!(indices_of(&r, "logs-1").len(), 1);
    r.clock.fetch_add(2 * DAY, Ordering::Relaxed);
    let dry = call(&r, "POST", "/logs-1/_rollover?dry_run=true", Some(body));
    let dry = json(&dry);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["rolled_over"], false);
    assert_eq!(dry["conditions"]["[max_age: 1d]"], true);
    assert_eq!(indices_of(&r, "logs-1").len(), 1);
    let old = call(&r, "POST", "/logs-1/_rollover", Some(body));
    assert_eq!(json(&old)["rolled_over"], true);
    assert_eq!(indices_of(&r, "logs-1").len(), 2);
}

// AC-3
#[test]
fn the_new_backing_index_has_the_templates_mapping_and_a_date_timestamp() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    assert_eq!(call(&r, "POST", "/logs-1/_rollover", None).status, 200);
    let mut buf = vec![0u8; BUF];
    let store = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let descriptors = DescriptorStore::new(store, "", DeleteMode::Conditional).expect("names");
    let record = descriptors
        .get(".ds-logs-1-000002", &mut buf)
        .expect("reads")
        .expect("the new backing index");
    let mapping: serde_json::Value =
        serde_json::from_str(record.mapping.as_deref().expect("a mapping")).expect("json");
    assert_eq!(mapping["properties"]["@timestamp"]["type"], "date");
    assert_eq!(mapping["properties"]["msg"]["type"], "text");
}

// AC-3
#[test]
fn a_taken_backing_name_is_moved_past_by_a_generation() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    let store = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let descriptors = DescriptorStore::new(store, "", DeleteMode::Conditional).expect("names");
    let mut buf = vec![0u8; BUF];
    let squatter = oss_format::IndexRecord::new(".ds-logs-1-000002", "squat", 1);
    descriptors
        .create(&squatter, 1, &mut buf)
        .expect("squatter");
    let rolled = json(&call(&r, "POST", "/logs-1/_rollover", None));
    assert_eq!(rolled["new_index"], ".ds-logs-1-000003");
    let got = json(&call(&r, "GET", "/_data_stream/logs-1", None));
    assert_eq!(got["data_streams"][0]["generation"], 3);
}

// AC-4
#[test]
fn an_alias_with_a_write_index_gains_the_new_one_as_its_write_index() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/app-000001", None).status, 200);
    let body = r#"{"is_write_index":true}"#;
    assert_eq!(
        call(&r, "PUT", "/app-000001/_alias/app", Some(body)).status,
        200
    );
    let rolled = json(&call(&r, "POST", "/app/_rollover", None));
    assert_eq!(rolled["old_index"], "app-000001");
    assert_eq!(rolled["new_index"], "app-000002");
    assert_eq!(rolled["rolled_over"], true);
    let members = json(&call(&r, "GET", "/_alias/app", None));
    assert!(
        members["app-000001"]["aliases"]["app"]
            .get("is_write_index")
            .is_none()
    );
    assert_eq!(
        members["app-000002"]["aliases"]["app"]["is_write_index"],
        true
    );
}

// AC-4
#[test]
fn an_alias_of_one_index_with_no_write_index_moves_to_the_new_one() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/app-000001", None).status, 200);
    assert_eq!(call(&r, "PUT", "/app-000001/_alias/app", None).status, 200);
    assert_eq!(call(&r, "POST", "/app/_rollover", None).status, 200);
    assert_eq!(
        json(&call(&r, "GET", "/_alias/app", None)),
        serde_json::json!({"app-000002": {"aliases": {"app": {}}}})
    );
    // A name with no trailing number needs the new name in the path.
    assert_eq!(call(&r, "PUT", "/plain", None).status, 200);
    assert_eq!(call(&r, "PUT", "/plain/_alias/p", None).status, 200);
    let refused = call(&r, "POST", "/p/_rollover", None);
    assert_eq!(refused.status, 400);
    assert!(
        json(&refused)["error"]["reason"]
            .as_str()
            .expect("reason")
            .contains("name the new index in the path")
    );
    let named = json(&call(&r, "POST", "/p/_rollover/plain-next", None));
    assert_eq!(named["new_index"], "plain-next");
    assert_eq!(
        json(&call(&r, "GET", "/_alias/p", None)),
        serde_json::json!({"plain-next": {"aliases": {"p": {}}}})
    );
}

// docs/specs/extension-analyzer AC-7: a rollover cannot add a pin; any
// settings in its body are refused and no index is created.
#[test]
fn a_rollover_body_cannot_carry_an_analyzer_pin() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    assert_eq!(call(&r, "PUT", "/idx-000001", None).status, 200);
    assert_eq!(call(&r, "PUT", "/idx-000001/_alias/a", None).status, 200);
    let hash = "0".repeat(64);
    let body = format!(
        r#"{{"settings":{{"index.analysis.analyzer.ext_stem.type":"serverless_module","index.analysis.analyzer.ext_stem.module":"{hash}","index.analysis.analyzer.ext_stem.version":"1"}}}}"#
    );
    for target in ["/logs-1/_rollover", "/a/_rollover"] {
        let reply = call(&r, "POST", target, Some(&body));
        assert!(
            reply.status == 400 || reply.status == 501,
            "{target}: {} {}",
            reply.status,
            reply.text()
        );
    }
    assert_eq!(call(&r, "HEAD", "/idx-000002", None).status, 404);
}

// AC-5
#[test]
fn what_cannot_roll_is_refused_and_creates_nothing() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    assert_eq!(call(&r, "PUT", "/idx-000001", None).status, 200);
    assert_eq!(call(&r, "POST", "/idx-000001/_rollover", None).status, 400);
    assert_eq!(call(&r, "POST", "/nothing/_rollover", None).status, 404);
    let named = call(&r, "POST", "/logs-1/_rollover/other", None);
    assert_eq!(named.status, 400);
    assert_eq!(kind(&named), "illegal_argument_exception");
    let shaped = call(
        &r,
        "POST",
        "/logs-1/_rollover",
        Some(r#"{"settings":{"index":{"number_of_shards":2}}}"#),
    );
    assert_eq!(shaped.status, 400);
    for (body, status) in [
        (r#"{"conditions":{"max_docs":5}}"#, 501),
        (r#"{"conditions":{"max_size":"1gb"}}"#, 501),
        (r#"{"conditions":{"nonsense":1}}"#, 400),
        (r#"{"aliases":{"a":{}}}"#, 400),
    ] {
        let reply = call(&r, "POST", "/logs-1/_rollover", Some(body));
        assert_eq!(reply.status, status, "{body}: {}", reply.text());
    }
    assert_eq!(call(&r, "POST", "/logs-*/_rollover", None).status, 501);
    // Several indices and none marked as the write index.
    for name in ["m-000001", "m-000002"] {
        assert_eq!(call(&r, "PUT", &format!("/{name}"), None).status, 200);
        assert_eq!(
            call(&r, "PUT", &format!("/{name}/_alias/m"), None).status,
            200
        );
    }
    let unwritable = call(&r, "POST", "/m/_rollover", None);
    assert_eq!(unwritable.status, 400);
    assert!(
        json(&unwritable)["error"]["reason"]
            .as_str()
            .expect("reason")
            .contains("does not point to a write index")
    );
    assert_eq!(indices_of(&r, "logs-1").len(), 1);
    assert_eq!(call(&r, "GET", "/_data_stream/logs-1", None).status, 200);
}

// AC-6
#[test]
fn the_requests_are_the_budget_rows() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    let at = requests(&r);
    assert_eq!(call(&r, "POST", "/logs-1/_rollover", None).status, 200);
    budget::check(
        "rollover.stream.requests",
        requests(&r) - at,
        "a rollover of a one-index stream",
    );
    assert_eq!(r.store.meter().count(S3Op::List), 0, "no LIST");
    assert_eq!(call(&r, "PUT", "/app-000001", None).status, 200);
    assert_eq!(
        call(
            &r,
            "PUT",
            "/app-000001/_alias/app",
            Some(r#"{"is_write_index":true}"#)
        )
        .status,
        200
    );
    let at = requests(&r);
    assert_eq!(call(&r, "POST", "/app/_rollover", None).status, 200);
    budget::check(
        "rollover.alias.requests",
        requests(&r) - at,
        "a rollover of an alias with a write index and one member",
    );
}

// AC-7
#[test]
fn a_swap_lost_to_another_change_removes_the_index_it_made_and_says_so() {
    let address: Arc<std::sync::Mutex<Option<std::net::SocketAddr>>> = Arc::default();
    let seen = Arc::clone(&address);
    // The new index joins a bucket while it is created, between the
    // rollover's read of the alias and its swap: change the alias there.
    let join: oss_node::index_route::BucketJoin = Arc::new(move |_, name, _| {
        if name == "app-000002" {
            let at = seen.lock().expect("lock").expect("address");
            let reply = support::request(at, "PUT", "/spare/_alias/app", None);
            assert_eq!(reply.status, 200, "{}", reply.text());
        }
        Ok("bucket".to_owned())
    });
    let r = rig_with(Some(join));
    *address.lock().expect("lock") = Some(r.server.local_addr());
    assert_eq!(call(&r, "PUT", "/app-000001", None).status, 200);
    assert_eq!(call(&r, "PUT", "/spare", None).status, 200);
    let body = r#"{"is_write_index":true}"#;
    assert_eq!(
        call(&r, "PUT", "/app-000001/_alias/app", Some(body)).status,
        200
    );
    let lost = call(&r, "POST", "/app/_rollover", None);
    assert_eq!(lost.status, 409, "{}", lost.text());
    assert_eq!(kind(&lost), "version_conflict_engine_exception");
    assert!(
        json(&lost)["error"]["reason"]
            .as_str()
            .expect("reason")
            .contains("the index [app-000002] created for it was removed"),
        "{}",
        lost.text()
    );
    let store = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let descriptors = DescriptorStore::new(store, "", DeleteMode::Conditional).expect("names");
    let mut buf = vec![0u8; BUF];
    assert!(
        descriptors
            .get("app-000002", &mut buf)
            .expect("reads")
            .is_none()
    );
    let members = json(&call(&r, "GET", "/_alias/app", None));
    assert!(members.get("app-000001").is_some() && members.get("spare").is_some());
    assert!(members.get("app-000002").is_none());
}
