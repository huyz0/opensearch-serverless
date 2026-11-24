//! docs/specs/snapshots-default-on: what a node started with no snapshot
//! variable serves of snapshots.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

use crate::support;

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn boot(root: &std::path::Path, snapshots: Option<&str>) -> Node {
    let root = root.display().to_string();
    let config = NodeConfig::from_lookup(|key| {
        Some(
            match key {
                "OSS_NODE_ID" => "n1",
                "OSS_STORE" => "fs",
                "OSS_FS_ROOT" => root.as_str(),
                "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                "OSS_SNAPSHOTS_ENABLED" => return snapshots.map(str::to_owned),
                _ => return None,
            }
            .to_owned(),
        )
    })
    .expect("valid");
    start(&config).expect("starts")
}

fn call(node: &Node, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(node.addr(), method, target, body.map(str::as_bytes))
}

// docs/specs/snapshots-default-on AC-1
#[test]
fn a_node_with_no_variable_serves_the_snapshot_reads_and_the_take_but_not_the_restore() {
    let dir = TempDir::new("boot-snapshots-default");
    let node = boot(dir.path(), None);
    let read = call(&node, "GET", "/_snapshot/backups", None);
    assert_eq!(read.status, 404, "{}", read.text());
    assert!(
        read.text().contains("repository_missing_exception"),
        "{}",
        read.text()
    );
    let take = call(
        &node,
        "PUT",
        "/_snapshot/backups/s1",
        Some(r#"{"indices":"logs"}"#),
    );
    assert_eq!(take.status, 404, "{}", take.text());
    assert!(
        take.text().contains("repository_missing_exception"),
        "{}",
        take.text()
    );
    let restore = call(&node, "POST", "/_snapshot/backups/s1/_restore", Some("{}"));
    assert_eq!(restore.status, 501, "{}", restore.text());
}

// docs/specs/snapshots-default-on AC-2
#[test]
fn the_variable_set_to_false_takes_the_snapshot_routes_away() {
    let dir = TempDir::new("boot-snapshots-off");
    let node = boot(dir.path(), Some("false"));
    let read = call(&node, "GET", "/_snapshot/backups", None);
    assert_eq!(read.status, 400, "{}", read.text());
}
