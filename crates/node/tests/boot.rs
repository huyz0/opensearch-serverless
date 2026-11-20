//! docs/specs/node-boot: a node started from configuration, over a file
//! store and real sockets.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

#[path = "support/mod.rs"]
mod support;

use std::time::{Duration, Instant};

use std::sync::Arc;

use oss_blob::FsStore;
use oss_cluster::{LeaseClock, LeaseConfig, Membership, SystemClock};
use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

/// The ids on the members index, read by a peer that is not a member.
fn enrolled(root: &std::path::Path) -> Vec<String> {
    let clock: Arc<dyn LeaseClock> = Arc::new(SystemClock);
    let observer = Membership::new(
        Arc::new(FsStore::new(root).expect("store")),
        "",
        clock,
        LeaseConfig::default(),
    )
    .expect("membership");
    let mut ids = observer.enrolled_ids().expect("reads");
    ids.sort();
    ids
}

fn config(root: &std::path::Path, id: &str) -> NodeConfig {
    let root = root.display().to_string();
    let lookup = |key: &str| {
        Some(
            match key {
                "OSS_NODE_ID" => id,
                "OSS_STORE" => "fs",
                "OSS_FS_ROOT" => root.as_str(),
                "OSS_HTTP_ADDR" => "127.0.0.1:0",
                // Fast enough for a test to see a peer; a third of it is the tick.
                "OSS_LEASE_TTL_MS" => "600",
                _ => return None,
            }
            .to_owned(),
        )
    };
    NodeConfig::from_lookup(lookup).expect("valid")
}

fn call(node: &Node, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(node.addr(), method, target, body.map(str::as_bytes))
}

fn nodes_seen(node: &Node) -> u64 {
    let health = call(node, "GET", "/_cluster/health", None);
    assert_eq!(health.status, 200, "{}", health.text());
    let text = health.text();
    let (_, rest) = text
        .split_once("\"number_of_nodes\":")
        .expect("reports nodes");
    rest.chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("a number")
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(20), "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// AC-3
#[test]
fn a_started_node_is_enrolled_and_reports_itself() {
    let dir = TempDir::new("boot-health");
    let node = start(&config(dir.path(), "n1")).expect("starts");
    assert_eq!(nodes_seen(&node), 1);
    assert_eq!(enrolled(dir.path()), ["n1"]);
    let health = call(&node, "GET", "/_cluster/health", None);
    assert!(
        health.text().contains("opensearch-serverless"),
        "{}",
        health.text()
    );
}

// AC-4
#[test]
fn a_started_node_writes_and_reads_a_document() {
    let dir = TempDir::new("boot-doc");
    let node = start(&config(dir.path(), "n1")).expect("starts");
    let made = call(&node, "PUT", "/logs", None);
    assert_eq!(made.status, 200, "{}", made.text());
    let wrote = call(&node, "PUT", "/logs/_doc/1", Some(r#"{"n":1}"#));
    assert_eq!(wrote.status, 201, "{}", wrote.text());
    assert_eq!(call(&node, "POST", "/logs/_refresh", None).status, 200);
    let read = call(&node, "GET", "/logs/_doc/1", None);
    assert_eq!(read.status, 200, "{}", read.text());
    assert!(read.text().contains("\"n\":1"), "{}", read.text());
}

// AC-5 and AC-6
#[test]
fn a_second_node_is_seen_and_a_dropped_one_is_released() {
    let dir = TempDir::new("boot-pair");
    let first = start(&config(dir.path(), "n1")).expect("starts");
    let second = start(&config(dir.path(), "n2")).expect("starts");
    assert_eq!(enrolled(dir.path()), ["n1", "n2"]);
    wait_for("the first never saw the second", || nodes_seen(&first) == 2);
    drop(second);
    // Released, not lapsed: the lease lasts 600 ms and the index is read at once.
    assert_eq!(enrolled(dir.path()), ["n1"]);
    wait_for("the first never saw the second leave", || {
        nodes_seen(&first) == 1
    });
}
