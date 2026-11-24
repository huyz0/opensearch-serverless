//! docs/specs/store-usage-wiring: a node's store usage over REST.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

use crate::support;

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn boot(root: &std::path::Path, usage: Option<&str>) -> Node {
    let root = root.display().to_string();
    let lookup = |key: &str| match key {
        "OSS_NODE_ID" => Some("n1".to_owned()),
        "OSS_STORE" => Some("fs".to_owned()),
        "OSS_FS_ROOT" => Some(root.clone()),
        "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
        "OSS_STORE_USAGE" => usage.map(str::to_owned),
        _ => None,
    };
    start(&NodeConfig::from_lookup(lookup).expect("valid")).expect("starts")
}

fn call(node: &Node, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(node.addr(), method, target, body.map(str::as_bytes))
}

// AC-2
#[test]
fn a_node_with_store_usage_on_charges_an_index_and_the_node() {
    let dir = TempDir::new("boot-usage");
    let node = boot(dir.path(), Some("true"));
    assert_eq!(call(&node, "PUT", "/logs", None).status, 200);
    assert_eq!(
        call(&node, "PUT", "/logs/_doc/1", Some(r#"{"n":1}"#)).status,
        201
    );
    assert_eq!(call(&node, "POST", "/logs/_refresh", None).status, 200);
    let usage = call(&node, "GET", "/_serverless/usage", None);
    assert_eq!(usage.status, 200, "{}", usage.text());
    let body = usage.text();
    assert!(body.contains("\"node_id\":\"n1\""), "{body}");
    assert!(body.contains("\"metered\":true"), "{body}");
    // The index's descriptor register was written for it; the leases and
    // members were the node's.
    assert!(body.contains("\"logs\":{\"cost_units\":"), "{body}");
    assert!(body.contains("\"_node\":{\"cost_units\":"), "{body}");
    assert!(body.contains("\"_other\":{\"cost_units\":0.000"), "{body}");
}

// AC-3
#[test]
fn a_node_without_store_usage_serves_no_usage_route() {
    let dir = TempDir::new("boot-usage-off");
    let node = boot(dir.path(), None);
    let reply = call(&node, "GET", "/_serverless/usage", None);
    assert_ne!(reply.status, 200, "{}", reply.text());
    assert!(!reply.text().contains("cost_units"));
}
