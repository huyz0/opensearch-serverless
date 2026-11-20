//! docs/specs/node-boot: a node started from configuration, over a file
//! store and real sockets.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

#[path = "support/mod.rs"]
mod support;

use std::time::{Duration, Instant};

use std::sync::Arc;

use oss_blob::{BlobStore, FsStore};
use oss_cluster::{LeaseClock, LeaseConfig, Membership, SystemClock};
use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

/// The write-log epochs a node's lease names, read from its register.
fn epochs(root: &std::path::Path, id: &str) -> Vec<i64> {
    let store = FsStore::new(root).expect("store");
    let key = oss_format::lease_key("", id).expect("key");
    let mut buf = vec![0u8; 1 << 16];
    let read = store
        .read_register(&key, &mut buf)
        .expect("reads")
        .expect("present");
    let lease = oss_format::NodeLease::parse(&buf[..read.len]).expect("parses");
    lease.node_log.iter().map(|e| e.epoch).collect()
}

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

fn config(root: &std::path::Path, id: &str, ttl: &str) -> NodeConfig {
    let root = root.display().to_string();
    let lookup = |key: &str| {
        Some(
            match key {
                "OSS_NODE_ID" => id,
                "OSS_STORE" => "fs",
                "OSS_FS_ROOT" => root.as_str(),
                "OSS_HTTP_ADDR" => "127.0.0.1:0",
                "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                // A third of it is the renewal tick.
                "OSS_LEASE_TTL_MS" => ttl,
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
    let node = start(&config(dir.path(), "n1", "30000")).expect("starts");
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
    let node = start(&config(dir.path(), "n1", "30000")).expect("starts");
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
    let first = start(&config(dir.path(), "n1", "3000")).expect("starts");
    let second = start(&config(dir.path(), "n2", "3000")).expect("starts");
    assert_eq!(enrolled(dir.path()), ["n1", "n2"]);
    wait_for("the first never saw the second", || nodes_seen(&first) == 2);
    drop(second);
    // Released, not lapsed: the lease lasts 3 s and the index is read at once.
    assert_eq!(enrolled(dir.path()), ["n1"]);
    // And nothing renews it again: a tick (1 s) passes and it is still gone.
    std::thread::sleep(Duration::from_millis(1_300));
    assert_eq!(enrolled(dir.path()), ["n1"]);
    assert_eq!(nodes_seen(&first), 1);
    wait_for("the first never saw the second leave", || {
        nodes_seen(&first) == 1
    });
}

// AC-8
#[test]
fn a_write_through_one_node_reaches_the_node_that_owns_the_shard() {
    let dir = TempDir::new("boot-forward");
    let first = start(&config(dir.path(), "n1", "30000")).expect("starts");
    let second = start(&config(dir.path(), "n2", "30000")).expect("starts");
    wait_for("the first never saw the second", || nodes_seen(&first) == 2);
    wait_for("the second never saw the first", || {
        nodes_seen(&second) == 2
    });
    let made = call(
        &first,
        "PUT",
        "/fwd",
        Some(r#"{"settings":{"index":{"number_of_shards":8}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    // Every id goes in through the second node; the shards it does not own
    // are forwarded to the first.
    for id in 0..24 {
        let wrote = call(
            &second,
            "PUT",
            &format!("/fwd/_doc/{id}?refresh=true"),
            Some(r#"{"n":1}"#),
        );
        assert_eq!(wrote.status, 201, "id {id}: {}", wrote.text());
    }
    // A read is served by the node that holds the shard (reads are not
    // forwarded): each document is on exactly one of the two, and both
    // hold some, so the first took writes the second received.
    let mut on_first = 0;
    let mut on_second = 0;
    for id in 0..24 {
        // Not realtime: only a refresh makes it visible.
        let target = format!("/fwd/_doc/{id}?realtime=false");
        let one = call(&first, "GET", &target, None);
        let two = call(&second, "GET", &target, None);
        assert_eq!(
            [one.status, two.status]
                .iter()
                .filter(|s| **s == 200)
                .count(),
            1,
            "id {id}: {} / {}",
            one.text(),
            two.text()
        );
        on_first += usize::from(one.status == 200);
        on_second += usize::from(two.status == 200);
    }
    assert!(on_first > 0 && on_second > 0, "{on_first} / {on_second}");
}

// AC-7
#[test]
fn a_restarted_node_writes_above_the_epochs_it_left() {
    let dir = TempDir::new("boot-restart");
    let write = |node: &Node, index: &str| {
        let r = call(node, "PUT", &format!("/{index}/_doc/1"), Some(r#"{"n":1}"#));
        assert_eq!(r.status, 201, "{}", r.text());
    };
    let first = start(&config(dir.path(), "n1", "30000")).expect("starts");
    assert_eq!(call(&first, "PUT", "/logs", None).status, 200);
    write(&first, "logs");
    let before = epochs(dir.path(), "n1");
    assert!(!before.is_empty(), "the first write opened an epoch");
    drop(first);
    // Released: off the members index, but its register still names the epochs.
    assert_eq!(enrolled(dir.path()), Vec::<String>::new());
    let second = start(&config(dir.path(), "n1", "30000")).expect("starts");
    assert_eq!(
        epochs(dir.path(), "n1"),
        before,
        "enrolling kept the epochs"
    );
    // A shard with a history cannot be taken over yet, so the new process
    // writes a shard of its own.
    assert_eq!(call(&second, "PUT", "/fresh", None).status, 200);
    write(&second, "fresh");
    let after = epochs(dir.path(), "n1");
    assert!(
        before.iter().all(|e| after.contains(e)),
        "{before:?} not all in {after:?}"
    );
    assert!(
        after.iter().max() > before.iter().max(),
        "the new process wrote in a new epoch: {after:?}"
    );
}
