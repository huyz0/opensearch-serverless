//! docs/specs/snapshot-take: a shallow snapshot taken over REST, answered as
//! the shell answers, with the request cost the budget table holds.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{FileLocation, IndexRecord, ManifestEntry, ShardRef, encode_manifest};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::snapshot_route::{SnapshotService, routes};
use oss_node::snapshot_take_route::SnapshotTakeService;
use oss_rest::{Router, Server};
use oss_testkit::budget;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
    descriptors: DescriptorStore,
}

fn roomy() -> BudgetConfig {
    BudgetConfig::new(256 << 20)
        .with_pool(Pool::RequestBodies, 64 << 20)
        .expect("fits")
}

fn rig(take: bool) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let clock: oss_meta::PitClock = Arc::new(|| 1_700_000_000_000);
    let service = Arc::new(SnapshotService::new(
        Arc::clone(&blobs),
        "",
        "node-1",
        Arc::clone(&clock),
    ));
    let descriptors =
        DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap();
    let taker = take.then(|| {
        Arc::new(SnapshotTakeService::new(
            Arc::clone(&blobs),
            "",
            DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap(),
            clock,
        ))
    });
    let router = routes(Router::new(), &service, taker.as_ref()).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&roomy());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        store,
        descriptors,
    }
}

fn send(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

fn repository(r: &Rig, shallow: bool) {
    let body = format!(
        r#"{{"type":"fs","settings":{{"location":"x","remote_store_index_shallow_copy":{shallow}}}}}"#
    );
    assert_eq!(
        send(r, "PUT", "/_snapshot/backups", Some(&body)).status,
        200
    );
}

fn index(r: &Rig, name: &str, uuid: &str, shards: u32) {
    r.descriptors
        .create(
            &IndexRecord::new(name, uuid, shards),
            1,
            &mut vec![0u8; 1 << 16],
        )
        .unwrap();
}

fn publish(r: &Rig, index: &str, uuid: &str, shard: u32) {
    let at = ShardRef { index, uuid, shard };
    let entries = [ManifestEntry {
        name: "_0.si",
        location: FileLocation::Whole { term_dir: "t=1" }.encode(),
        length: 7,
    }];
    let value = encode_manifest(1, Some("n1"), &entries, -1, -1).unwrap();
    r.store
        .create_register(&at.manifest_key("").unwrap(), &value)
        .unwrap();
}

fn requests(r: &Rig) -> u64 {
    S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum()
}

const TAKE: &str = r#"{"indices":"logs"}"#;

#[test]
fn a_take_is_accepted_and_the_snapshot_reads_back_as_captured() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 2);
    publish(&r, "logs", "u1", 0);
    publish(&r, "logs", "u1", 1);
    let put = send(&r, "PUT", "/_snapshot/backups/daily", Some(TAKE));
    assert_eq!(
        (put.status, put.text().as_str()),
        (200, r#"{"accepted":true}"#)
    );
    let got = send(&r, "GET", "/_snapshot/backups/daily", None);
    assert_eq!(got.status, 200, "{}", got.text());
    assert!(
        got.text().contains(r#""state":"SUCCESS""#),
        "{}",
        got.text()
    );
    assert!(got.text().contains(r#""logs""#), "{}", got.text());
    let again = send(&r, "POST", "/_snapshot/backups/daily", Some(TAKE));
    assert_eq!(again.status, 400, "{}", again.text());
    assert!(
        again.text().contains("snapshot_already_exists"),
        "{}",
        again.text()
    );
}

#[test]
fn wait_for_completion_answers_the_snapshot() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish(&r, "logs", "u1", 0);
    let put = send(
        &r,
        "PUT",
        "/_snapshot/backups/daily?wait_for_completion=true",
        Some(TAKE),
    );
    assert_eq!(put.status, 200, "{}", put.text());
    assert!(
        put.text().contains(r#""state":"SUCCESS""#),
        "{}",
        put.text()
    );
    assert!(
        put.text().contains(r#""snapshot":"daily""#),
        "{}",
        put.text()
    );
}

#[test]
fn the_refusals_have_the_shells_status_and_name() {
    let r = rig(true);
    let case = |target: &str, body: &str, status: u16, kind: &str| {
        let got = send(&r, "PUT", target, Some(body));
        assert_eq!(got.status, status, "{target} {body}: {}", got.text());
        assert!(got.text().contains(kind), "{target} {body}: {}", got.text());
    };
    case(
        "/_snapshot/nope/s",
        TAKE,
        404,
        "repository_missing_exception",
    );
    repository(&r, false);
    case(
        "/_snapshot/backups/s",
        TAKE,
        501,
        "unsupported_snapshot_storage",
    );
    repository(&r, true);
    case("/_snapshot/backups/s", TAKE, 404, "index_not_found");
    case(
        "/_snapshot/backups/s",
        r#"{"indices":"logs","ignore_unavailable":true}"#,
        404,
        "index_not_found",
    );
    case("/_snapshot/backups/s", "{}", 400, "missing_indices");
    case(
        "/_snapshot/backups/s",
        r#"{"indices":"a","metadata":{}}"#,
        501,
        "unsupported_snapshot_option",
    );
    case(
        "/_snapshot/backups/_s",
        TAKE,
        400,
        "invalid_snapshot_name_exception",
    );
    index(&r, "logs", "u1", 2);
    publish(&r, "logs", "u1", 0);
    case("/_snapshot/backups/s", TAKE, 409, "nothing_published");
    // The failed take left nothing: the name is free and reads as missing.
    assert_eq!(send(&r, "GET", "/_snapshot/backups/s", None).status, 404);
    publish(&r, "logs", "u1", 1);
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups/s", Some(TAKE)).status,
        200
    );
}

#[test]
fn partial_skips_an_index_with_an_unpublished_shard_and_all_skipped_is_refused() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "good", "u1", 1);
    publish(&r, "good", "u1", 0);
    index(&r, "late", "u2", 1);
    let body = r#"{"indices":["good","late"],"partial":true}"#;
    let put = send(
        &r,
        "PUT",
        "/_snapshot/backups/p?wait_for_completion=true",
        Some(body),
    );
    assert_eq!(put.status, 200, "{}", put.text());
    assert!(
        put.text().contains(r#""good""#) && !put.text().contains(r#""late""#),
        "{}",
        put.text()
    );
    let none = send(
        &r,
        "PUT",
        "/_snapshot/backups/q",
        Some(r#"{"indices":"late","partial":true}"#),
    );
    assert_eq!(none.status, 409, "{}", none.text());
    assert_eq!(send(&r, "GET", "/_snapshot/backups/q", None).status, 404);
}

#[test]
fn a_take_costs_what_the_spec_budgets() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish(&r, "logs", "u1", 0);
    let before = requests(&r);
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups/daily", Some(TAKE)).status,
        200
    );
    budget::check(
        "snapshot.take.requests",
        requests(&r) - before,
        "a one-index, one-shard take",
    );
    index(&r, "a", "ua", 2);
    index(&r, "b", "ub", 2);
    for (i, u) in [("a", "ua"), ("b", "ub")] {
        publish(&r, i, u, 0);
        publish(&r, i, u, 1);
    }
    let before = requests(&r);
    let two = r#"{"indices":["a","b"]}"#;
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups/two", Some(two)).status,
        200
    );
    budget::check(
        "snapshot.take_two.requests",
        requests(&r) - before,
        "a two-index, four-shard take",
    );
}

#[test]
fn with_the_take_switch_off_a_take_answers_501() {
    let r = rig(false);
    repository(&r, true);
    let got = send(&r, "PUT", "/_snapshot/backups/daily", Some(TAKE));
    assert_eq!(got.status, 501, "{}", got.text());
}
