//! docs/specs/search-queue AC-8, AC-9: the queue on a booted node.

use crate::support;

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn boot(root: &std::path::Path, extra: &[(&str, &str)]) -> Node {
    let root = root.display().to_string();
    let config = NodeConfig::from_lookup(|key| {
        if let Some((_, v)) = extra.iter().find(|(k, _)| *k == key) {
            return Some((*v).to_owned());
        }
        match key {
            "OSS_NODE_ID" => Some("n1".to_owned()),
            "OSS_STORE" => Some("fs".to_owned()),
            "OSS_FS_ROOT" => Some(root.clone()),
            "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
            "OSS_STORE_USAGE" => Some("true".to_owned()),
            _ => None,
        }
    })
    .expect("valid");
    start(&config).expect("starts")
}

fn call(node: &Node, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(node.addr(), method, target, body.map(str::as_bytes))
}

fn search_all(node: &Node) -> (String, String) {
    assert_eq!(call(node, "PUT", "/logs", None).status, 200);
    let bulk = "{\"index\":{\"_id\":\"1\"}}\n{\"msg\":\"hello\"}\n";
    let put = call(node, "POST", "/logs/_bulk?refresh=true", Some(bulk));
    assert_eq!(put.status, 200, "{}", put.text());
    let found = call(
        node,
        "POST",
        "/logs/_search",
        Some(r#"{"query":{"match_all":{}}}"#),
    );
    assert_eq!(found.status, 200, "{}", found.text());
    (
        found.text(),
        call(node, "GET", "/_serverless/usage", None).text(),
    )
}

// AC-8, AC-9
#[test]
fn a_node_with_the_queue_on_serves_searches_and_reports_it_and_one_without_does_not() {
    let dir = TempDir::new("boot-search-queue");
    let off = boot(&dir.path().join("off"), &[]);
    let (answer_off, usage_off) = search_all(&off);
    assert!(!usage_off.contains("search_queue"), "{usage_off}");
    drop(off);
    let on = boot(&dir.path().join("on"), &[("OSS_SEARCH_QUEUE_SIZE", "10")]);
    let (answer_on, usage_on) = search_all(&on);
    assert!(
        usage_on.contains("\"search_queue\":{\"running\":0,\"parked\":0"),
        "{usage_on}"
    );
    let hits = |t: &str| t.split_once("\"hits\"").map(|(_, h)| h.to_owned());
    assert_eq!(hits(&answer_off), hits(&answer_on));
}
