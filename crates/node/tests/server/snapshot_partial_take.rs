//! docs/specs/snapshot-partial AC-5 to AC-8: a `partial` take keeps an index
//! whose shard published nothing, behind `serverless.snapshots.take.partial_shards`.

use crate::snapshot_take::{Rig, index, publish, repository, requests, send};
use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::snapshot_route::{SnapshotService, routes};
use oss_node::snapshot_take_route::SnapshotTakeService;
use oss_rest::{Router, Server};
use oss_testkit::budget;

fn rig(partial_shards: bool) -> Rig {
    let config = BudgetConfig::new(256 << 20)
        .with_pool(Pool::RequestBodies, 64 << 20)
        .expect("fits");
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
    let taker = Arc::new(
        SnapshotTakeService::new(
            Arc::clone(&blobs),
            "",
            DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap(),
            clock,
        )
        .with_partial_shards(partial_shards),
    );
    let router = routes(Router::new(), &service, Some(&taker)).expect("routes");
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

fn partial_take(r: &Rig, name: &str, body: &str) -> support::Reply {
    send(
        r,
        "PUT",
        &format!("/_snapshot/backups/{name}?wait_for_completion=true"),
        Some(body),
    )
}

// AC-5: with the setting on, the shard that published nothing is recorded as failed.
#[test]
fn a_partial_take_keeps_the_index_and_names_the_shard_that_published_nothing() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 3);
    publish(&r, "logs", "u1", 0);
    publish(&r, "logs", "u1", 2);
    let put = partial_take(&r, "p", r#"{"indices":"logs","partial":true}"#);
    assert_eq!(put.status, 200, "{}", put.text());
    let got = send(&r, "GET", "/_snapshot/backups/p", None);
    let text = got.text();
    assert!(text.contains(r#""state":"PARTIAL""#), "{text}");
    assert!(text.contains(r#""shard_id":1"#), "{text}");
    assert!(
        text.contains(r#""shards":{"total":3,"failed":1,"successful":2}"#),
        "{text}"
    );
}

// AC-6: without `partial`, or with the setting off, nothing changes.
#[test]
fn without_partial_or_the_setting_a_shard_that_published_nothing_is_as_before() {
    let on = rig(true);
    repository(&on, true);
    index(&on, "logs", "u1", 2);
    publish(&on, "logs", "u1", 0);
    let plain = partial_take(&on, "a", r#"{"indices":"logs"}"#);
    assert_eq!(plain.status, 409, "{}", plain.text());
    assert!(
        plain.text().contains("nothing_published"),
        "{}",
        plain.text()
    );

    let off = rig(false);
    repository(&off, true);
    index(&off, "logs", "u1", 2);
    publish(&off, "logs", "u1", 0);
    index(&off, "good", "u2", 1);
    publish(&off, "good", "u2", 0);
    let skipped = partial_take(&off, "b", r#"{"indices":["logs","good"],"partial":true}"#);
    assert_eq!(skipped.status, 200, "{}", skipped.text());
    let text = send(&off, "GET", "/_snapshot/backups/b", None).text();
    assert!(
        text.contains(r#""state":"SUCCESS""#) && !text.contains(r#""logs""#),
        "{text}"
    );
}

// AC-7: an index whose every shard failed appears only beside one that captured a shard.
#[test]
fn an_all_failed_index_needs_an_index_that_captured_a_shard() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "dead", "u1", 2);
    let alone = partial_take(&r, "x", r#"{"indices":"dead","partial":true}"#);
    assert_eq!(alone.status, 409, "{}", alone.text());
    index(&r, "good", "u2", 1);
    publish(&r, "good", "u2", 0);
    let both = partial_take(&r, "y", r#"{"indices":["dead","good"],"partial":true}"#);
    assert_eq!(both.status, 200, "{}", both.text());
    let text = send(&r, "GET", "/_snapshot/backups/y", None).text();
    assert!(text.contains(r#""state":"PARTIAL""#), "{text}");
    assert!(
        text.contains(r#""shards":{"total":3,"failed":2,"successful":1}"#),
        "{text}"
    );
}

// AC-8: the cost of a partial take, k of n shards without a commit.
#[test]
fn a_partial_take_costs_what_the_budget_holds() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 3);
    publish(&r, "logs", "u1", 0);
    let before = requests(&r);
    let put = partial_take(&r, "c", r#"{"indices":"logs","partial":true}"#);
    assert_eq!(put.status, 200, "{}", put.text());
    budget::check(
        "snapshot.take_partial.requests",
        requests(&r) - before,
        "a take of a three-shard index in which two shards published nothing",
    );
}
