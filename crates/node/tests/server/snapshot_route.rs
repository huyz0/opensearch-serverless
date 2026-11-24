//! The snapshot read routes over a node's store
//! (docs/specs/snapshot-read/SPEC.md).

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::snapshot_route::{SnapshotService, routes};
use oss_rest::{Router, Server};
use oss_testkit::budget;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    rig_with(support::budget_config())
}

fn rig_with(config: BudgetConfig) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(SnapshotService::new(
        blobs,
        "",
        "node-1",
        Arc::new(|| 1_700_000_000_000),
    ));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&config);
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

fn call(r: &Rig, method: &str, target: &str) -> support::Reply {
    support::request(r.server.local_addr(), method, target, None)
}

#[allow(clippy::disallowed_methods)] // a fixture a few hundred bytes long
fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/snapshots")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A budget whose request-body pool holds a snapshot record's buffer.
fn roomy() -> BudgetConfig {
    BudgetConfig::new(256 << 20)
        .with_pool(Pool::RequestBodies, 64 << 20)
        .expect("fits")
}

fn seed(r: &Rig) {
    r.store
        .put(
            "repositories/backups",
            &mut SliceSource::new(&fixture("repository.json")),
            &mut [0u8; 256],
        )
        .expect("repository");
    let key = oss_format::snapshot_state_key("", "backups", "daily-1").expect("key");
    r.store
        .create_register(&key, &fixture("state-finished.bin"))
        .expect("state");
}

#[test]
fn a_repository_and_a_snapshot_are_read_in_one_request_each() {
    let r = rig_with(roomy());
    seed(&r);
    let before = r.store.meter().count(S3Op::Get);
    let repo = call(&r, "GET", "/_snapshot/backups");
    assert_eq!(repo.status, 200, "{}", repo.text());
    assert!(repo.text().contains("\"backups\""), "{}", repo.text());
    assert_eq!(r.store.meter().count(S3Op::Get) - before, 1);
    let before = r.store.meter().count(S3Op::Get);
    let snap = call(&r, "GET", "/_snapshot/backups/daily-1");
    assert_eq!(snap.status, 200, "{}", snap.text());
    assert!(snap.text().contains("\"daily-1\""), "{}", snap.text());
    assert_eq!(r.store.meter().count(S3Op::Get) - before, 1);
    assert_eq!(call(&r, "GET", "/_snapshot").status, 200);
}

#[test]
fn a_missing_repository_or_snapshot_is_404_and_a_selector_or_bad_name_is_refused() {
    let r = rig_with(roomy());
    seed(&r);
    assert_eq!(call(&r, "GET", "/_snapshot/nope").status, 404);
    assert_eq!(call(&r, "GET", "/_snapshot/backups/nope").status, 404);
    assert_eq!(call(&r, "GET", "/_snapshot/backups/_all").status, 501);
    assert_eq!(call(&r, "GET", "/_snapshot/backups/a,b").status, 501);
    assert_eq!(call(&r, "GET", "/_snapshot/backups/_x").status, 400);
    assert_eq!(call(&r, "GET", "/_snapshot/_all").status, 501);
    assert_eq!(call(&r, "GET", "/_snapshot/a*/daily-1").status, 501);
}

#[test]
fn a_snapshot_write_route_answers_501() {
    let r = rig();
    for (m, t) in [
        ("PUT", "/_snapshot/backups/daily-2"),
        ("POST", "/_snapshot/backups/daily-1"),
        ("DELETE", "/_snapshot/backups/daily-1"),
    ] {
        assert_eq!(call(&r, m, t).status, 501, "{m} {t}");
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

#[test]
fn a_repository_is_registered_read_back_replaced_and_removed() {
    let r = rig_with(roomy());
    let put = send(
        &r,
        "PUT",
        "/_snapshot/backups",
        Some(r#"{"type":"fs","settings":{"location":"x","max":5}}"#),
    );
    assert_eq!(
        (put.status, put.text()),
        (200, r#"{"acknowledged":true}"#.to_owned())
    );
    let got = call(&r, "GET", "/_snapshot/backups");
    assert_eq!(got.status, 200, "{}", got.text());
    assert!(
        got.text().contains(r#""location":"x""#) && got.text().contains(r#""max":"5""#),
        "{}",
        got.text()
    );
    let again = send(
        &r,
        "POST",
        "/_snapshot/backups",
        Some(r#"{"settings":{"location":"y"}}"#),
    );
    assert_eq!(again.status, 200);
    assert!(
        call(&r, "GET", "/_snapshot/backups")
            .text()
            .contains(r#""location":"y""#)
    );
    assert_eq!(send(&r, "DELETE", "/_snapshot/backups", None).status, 200);
    assert_eq!(call(&r, "GET", "/_snapshot/backups").status, 404);
    assert_eq!(send(&r, "DELETE", "/_snapshot/backups", None).status, 404);
}

#[test]
fn a_registration_the_shell_refuses_is_refused_before_any_write() {
    let r = rig_with(roomy());
    let puts = r.store.meter().count(S3Op::Put);
    for (target, body, status) in [
        ("/_snapshot/_x", "{}", 501),
        ("/_snapshot/_all", "{}", 400),
        ("/_snapshot/a%23b", "{}", 400),
        ("/_snapshot/r", r#"{"type":"s3"}"#, 501),
        ("/_snapshot/r", r#"{"crypto_settings":{"k":"v"}}"#, 501),
        ("/_snapshot/r", "[]", 400),
        ("/_snapshot/r", r#"{"settings":{"a":[1]}}"#, 400),
    ] {
        assert_eq!(
            send(&r, "PUT", target, Some(body)).status,
            status,
            "{target} {body}"
        );
    }
    assert_eq!(
        r.store.meter().count(S3Op::Put),
        puts,
        "nothing was written"
    );
}

#[test]
fn a_repository_holding_a_snapshot_is_not_removed_and_a_delete_validates_every_name() {
    let r = rig_with(roomy());
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups", Some("{}")).status,
        200
    );
    let legacy = fixture("snapshot-finished.json");
    r.store
        .put(
            "snapshots/backups#daily-1",
            &mut SliceSource::new(&legacy),
            &mut [0u8; 256],
        )
        .expect("marker");
    let in_use = send(&r, "DELETE", "/_snapshot/backups", None);
    assert_eq!(in_use.status, 409, "{}", in_use.text());
    assert_eq!(call(&r, "GET", "/_snapshot/backups").status, 200);
    for target in ["/_snapshot/_all", "/_snapshot/backups,_x", "/_snapshot/a*"] {
        assert_eq!(send(&r, "DELETE", target, None).status, 400, "{target}");
    }
    assert_eq!(call(&r, "GET", "/_snapshot/backups").status, 200);
}

#[test]
fn verify_answers_this_node_for_a_registered_repository() {
    let r = rig_with(roomy());
    seed(&r);
    let ok = send(&r, "POST", "/_snapshot/backups/_verify", None);
    assert_eq!(
        (ok.status, ok.text()),
        (200, r#"{"nodes":{"node-1":{"name":"node-1"}}}"#.to_owned())
    );
    assert_eq!(
        send(&r, "POST", "/_snapshot/nope/_verify", None).status,
        404
    );
    assert_eq!(send(&r, "POST", "/_snapshot/_x/_verify", None).status, 400);
}

#[test]
fn repository_writes_cost_what_the_spec_budgets() {
    let r = rig_with(roomy());
    let before = r.store.meter().total();
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups", Some("{}")).status,
        200
    );
    budget::check(
        "repository.put.requests",
        r.store.meter().total().saturating_sub(before),
        "a registration",
    );
    let before = r.store.meter().total();
    assert_eq!(
        send(&r, "POST", "/_snapshot/backups/_verify", None).status,
        200
    );
    budget::check(
        "repository.verify.requests",
        r.store.meter().total().saturating_sub(before),
        "a verification",
    );
    let before = r.store.meter().total();
    assert_eq!(send(&r, "DELETE", "/_snapshot/backups", None).status, 200);
    budget::check(
        "repository.delete_empty.requests",
        r.store.meter().total().saturating_sub(before),
        "removing an empty repository",
    );
}

#[test]
fn a_snapshot_read_the_body_pool_cannot_hold_is_429_and_a_repository_read_still_serves() {
    let r = rig();
    seed(&r);
    assert_eq!(call(&r, "GET", "/_snapshot/backups/daily-1").status, 429);
    assert_eq!(call(&r, "GET", "/_snapshot/backups").status, 200);
}

fn gets(r: &Rig, target: &str) -> (u16, u64) {
    let before = r.store.meter().total();
    let reply = call(r, "GET", target);
    (reply.status, r.store.meter().total().saturating_sub(before))
}

#[test]
fn snapshot_reads_cost_what_the_spec_budgets() {
    let r = rig_with(roomy());
    seed(&r);
    let (status, n) = gets(&r, "/_snapshot/backups");
    assert_eq!(status, 200);
    budget::check("snapshot.repo.get.requests", n, "one repository");
    let (status, n) = gets(&r, "/_snapshot/backups/daily-1");
    assert_eq!(status, 200);
    budget::check("snapshot.get.requests", n, "a register snapshot");
    let (status, n) = gets(&r, "/_snapshot/backups/daily-9");
    assert_eq!(status, 404);
    budget::check("snapshot.get_missing.requests", n, "a missing snapshot");
    let (status, n) = gets(&r, "/_snapshot");
    assert_eq!(status, 200);
    budget::check("snapshot.repos.list.requests", n, "one repository listed");

    let legacy = rig_with(roomy());
    legacy
        .store
        .put(
            "snapshots/backups#daily-1",
            &mut SliceSource::new(&fixture("snapshot-finished.json")),
            &mut [0u8; 256],
        )
        .expect("legacy record");
    let (status, n) = gets(&legacy, "/_snapshot/backups/daily-1");
    assert_eq!(status, 200);
    budget::check("snapshot.get_legacy.requests", n, "a legacy snapshot");
}
