//! docs/specs/snapshot-take: a shallow snapshot taken over REST, answered as
//! the shell answers, with the request cost the budget table holds.

// The fixture the Java reads is a few hundred bytes.
#![allow(clippy::disallowed_methods)]

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

pub(crate) struct Rig {
    pub(crate) server: Server,
    pub(crate) store: Arc<Emulator>,
    pub(crate) descriptors: DescriptorStore,
}

fn roomy() -> BudgetConfig {
    BudgetConfig::new(256 << 20)
        .with_pool(Pool::RequestBodies, 64 << 20)
        .expect("fits")
}

pub(crate) fn rig(take: bool) -> Rig {
    rig_in(take, roomy())
}

fn rig_in(take: bool, config: BudgetConfig) -> Rig {
    rig_with(take, config, "")
}

fn rig_with(take: bool, config: BudgetConfig, system: &str) -> Rig {
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
        Arc::new(
            SnapshotTakeService::new(
                Arc::clone(&blobs),
                "",
                DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap(),
                clock,
            )
            .with_system_indices(Arc::new(
                oss_coordinator::system_indices::SystemIndices::parse(system),
            )),
        )
    });
    let router = routes(Router::new(), &service, taker.as_ref()).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&config);
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        store,
        descriptors,
    }
}

pub(crate) fn send(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

pub(crate) fn repository(r: &Rig, shallow: bool) {
    let body = format!(
        r#"{{"type":"fs","settings":{{"location":"x","remote_store_index_shallow_copy":{shallow}}}}}"#
    );
    assert_eq!(
        send(r, "PUT", "/_snapshot/backups", Some(&body)).status,
        200
    );
}

pub(crate) fn index(r: &Rig, name: &str, uuid: &str, shards: u32) {
    r.descriptors
        .create(
            &IndexRecord::new(name, uuid, shards),
            1,
            &mut vec![0u8; 1 << 16],
        )
        .unwrap();
}

pub(crate) fn publish(r: &Rig, index: &str, uuid: &str, shard: u32) {
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

pub(crate) fn requests(r: &Rig) -> u64 {
    S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum()
}

pub(crate) const TAKE: &str = r#"{"indices":"logs"}"#;

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

/// Publishes a commit holding one file in each kind of location a writer uses.
fn publish_mixed(r: &Rig, index: &str, uuid: &str, shard: u32) {
    let at = ShardRef { index, uuid, shard };
    let entries = [
        ManifestEntry {
            name: "segments_1",
            location: FileLocation::Whole { term_dir: "t=1" }.encode(),
            length: 7,
        },
        ManifestEntry {
            name: "_0.cfs",
            location: FileLocation::Packed {
                term_dir: "t=1",
                blob: "s0",
                offset: 10,
                length: 5,
            }
            .encode(),
            length: 5,
        },
        ManifestEntry {
            name: "_0.si",
            location: FileLocation::NodePacked {
                node: "nodeA",
                epoch: 3,
                ordinal: 2,
                offset: 40,
                length: 9,
            }
            .encode(),
            length: 9,
        },
    ];
    let value = encode_manifest(4, Some("n1"), &entries, -1, -1).unwrap();
    r.store
        .create_register(&at.manifest_key("").unwrap(), &value)
        .unwrap();
}

/// The register value and marker blob a Rust take wrote, for the Java to read
/// (`fixtures/snapshots/rust-take/`, `Check.java`). With `OSS_WRITE_FIXTURES`
/// set the files are rewritten; otherwise the test checks they still parse.
#[test]
fn a_take_writes_the_register_and_marker_a_java_node_reads() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "uuidlogs0000000000000A", 2);
    publish_mixed(&r, "logs", "uuidlogs0000000000000A", 0);
    publish_mixed(&r, "logs", "uuidlogs0000000000000A", 1);
    let body = r#"{"indices":"logs"}"#;
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups/daily-1", Some(body)).status,
        200
    );
    let key = oss_format::snapshot_state_key("", "backups", "daily-1").unwrap();
    let mut state = vec![0u8; 1 << 20];
    let read = r.store.read_register(&key, &mut state).unwrap().unwrap();
    state.truncate(read.len);
    let oss_format::SnapshotState::Held {
        marker,
        record,
        finished,
    } = oss_format::SnapshotState::parse(&state).unwrap()
    else {
        panic!("held");
    };
    assert!(finished);
    let mut marker_body = Vec::new();
    r.store
        .get(
            &format!("snapshots/{marker}"),
            oss_blob::ByteRange::Full,
            &mut [0u8; 4096],
            &mut |c: &[u8]| {
                marker_body.extend_from_slice(c);
                Ok(())
            },
        )
        .unwrap();
    let summary = oss_format::SnapshotSummary::parse(record).unwrap();
    assert_eq!(
        (summary.indices[0].shards, summary.indices[0].captured),
        (2, 2)
    );
    let marker_summary = oss_format::SnapshotSummary::parse(&marker_body).unwrap();
    assert_eq!(
        marker_summary.indices[0].captured, 0,
        "the marker names no commits"
    );
    assert_eq!(marker_summary.end_millis, marker_summary.start_millis);
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/snapshots/rust-take");
    if std::env::var_os("OSS_WRITE_FIXTURES").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("state.bin"), &state).unwrap();
        std::fs::write(dir.join("marker.json"), &marker_body).unwrap();
    }
    let committed = std::fs::read(dir.join("state.bin")).expect("run with OSS_WRITE_FIXTURES=1");
    assert!(matches!(
        oss_format::SnapshotState::parse(&committed),
        Ok(oss_format::SnapshotState::Held { finished: true, .. })
    ));
}

/// What the janitor would pin for the deployment's snapshots.
pub(crate) fn pinned(r: &Rig, index: &str, uuid: &str) -> bool {
    let source = oss_janitor::orphans::SnapshotSource::new(
        Arc::clone(&r.store) as Arc<dyn BlobStore>,
        "",
        100,
        1 << 20,
    );
    let mut pins = oss_janitor::orphans::IndexPins::new(100);
    source
        .collect_with(&mut pins, &mut vec![0u8; 1 << 20])
        .expect("reads");
    pins.contains(index, uuid)
}

#[test]
fn the_janitor_pins_an_index_from_the_protective_marker_and_after_the_finish() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish(&r, "logs", "u1", 0);
    assert!(!pinned(&r, "logs", "u1"), "no snapshot, no pin");
    // The marker alone, before any commit is read.
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let taker = oss_meta::SnapshotTaker::new(Arc::clone(&blobs), "");
    let described = taker
        .describe(&r.descriptors, "logs", &mut vec![0u8; 1 << 16])
        .unwrap();
    let draft = |captured: bool| oss_format::SnapshotDraft {
        repo: "backups".into(),
        name: "held".into(),
        uuid: "snap".into(),
        storage_generation: Some("genAAAAAAAAAAAAAAAAAAA".into()),
        shallow: true,
        start_millis: 1,
        end_millis: 1,
        global_state: None,
        indices: vec![described.capture(Vec::new(), captured)],
    };
    let mut value = vec![0u8; 1 << 16];
    let reservation = taker.reserve(&draft(true), &mut value).unwrap();
    assert!(
        !pinned(&r, "logs", "u1"),
        "a reservation alone has no marker"
    );
    taker
        .publish_marker(&reservation, &draft(false), &mut value)
        .unwrap();
    assert!(pinned(&r, "logs", "u1"), "pinned from the marker");
    taker.abort(&reservation, &mut value).unwrap();
    assert!(!pinned(&r, "logs", "u1"), "an aborted take pins nothing");
    // A whole take through the route.
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups/daily", Some(TAKE)).status,
        200
    );
    assert!(pinned(&r, "logs", "u1"), "pinned after the finish");
    assert!(!pinned(&r, "logs", "other-uuid"), "only that incarnation");
}

#[test]
fn a_take_the_body_pool_cannot_hold_is_429_and_makes_no_write() {
    let r = rig_in(true, support::budget_config());
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish(&r, "logs", "u1", 0);
    let before = requests(&r);
    let got = send(&r, "PUT", "/_snapshot/backups/daily", Some(TAKE));
    assert_eq!(got.status, 429, "{}", got.text());
    assert_eq!(requests(&r), before, "nothing is read or written");
    assert_eq!(send(&r, "GET", "/_snapshot/backups", None).status, 200);
}

#[test]
fn with_the_take_switch_off_a_take_answers_501() {
    let r = rig(false);
    repository(&r, true);
    let got = send(&r, "PUT", "/_snapshot/backups/daily", Some(TAKE));
    assert_eq!(got.status, 501, "{}", got.text());
}

#[test]
fn a_system_index_is_not_captured_even_when_unavailable_is_ignored() {
    let r = rig_with(true, roomy(), ".kibana*");
    repository(&r, true);
    index(&r, ".kibana_1", "u1", 1);
    publish(&r, ".kibana_1", "u1", 0);
    for body in [
        r#"{"indices":".kibana_1"}"#,
        r#"{"indices":".kibana_1","ignore_unavailable":true}"#,
    ] {
        let reply = send(&r, "PUT", "/_snapshot/backups/daily", Some(body));
        assert_eq!(reply.status, 403, "{body}: {}", reply.text());
        assert!(reply.text().contains("system_index"), "{}", reply.text());
    }
    assert!(!pinned(&r, ".kibana_1", "u1"), "nothing was captured");
}
