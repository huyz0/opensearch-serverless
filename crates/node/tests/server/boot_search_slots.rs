//! docs/specs/search-slots: the gate on a booted node.

use crate::support;

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn config(root: &str, extra: &[(&str, &str)]) -> Result<NodeConfig, String> {
    NodeConfig::from_lookup(|key| {
        if let Some((_, v)) = extra.iter().find(|(k, _)| *k == key) {
            return Some((*v).to_owned());
        }
        match key {
            "OSS_NODE_ID" => Some("n1".to_owned()),
            "OSS_STORE" => Some("fs".to_owned()),
            "OSS_FS_ROOT" => Some(root.to_owned()),
            "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
            "OSS_STORE_USAGE" => Some("true".to_owned()),
            _ => None,
        }
    })
}

fn boot(root: &std::path::Path, extra: &[(&str, &str)]) -> Node {
    let root = root.display().to_string();
    start(&config(&root, extra).expect("valid")).expect("starts")
}

fn call(node: &Node, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(node.addr(), method, target, body.map(str::as_bytes))
}

// AC-10
#[test]
fn the_settings_default_to_off_and_refuse_what_leaves_no_headroom() {
    let off = config("/tmp/unused", &[]).expect("valid").search_slots;
    assert_eq!(
        (off.slots, off.fair_share_pct, off.wait_millis),
        (0, 25, 1_000)
    );
    assert_eq!(off.waiter_cap, 64);
    for (key, value) in [
        ("OSS_SEARCH_SLOTS", "soon"),
        ("OSS_SEARCH_SLOTS", "10001"),
        ("OSS_SEARCH_FAIR_SHARE_PCT", "0"),
        ("OSS_SEARCH_FAIR_SHARE_PCT", "101"),
        ("OSS_SEARCH_WAIT_MS", "60001"),
        ("OSS_SEARCH_MAX_WAITERS_PCT", "101"),
    ] {
        assert!(
            config("/tmp/unused", &[(key, value)]).is_err(),
            "{key}={value}"
        );
    }
    // 128 workers: 64 slots and a cap of 64 leave none; 63 and 64 leave one.
    assert!(config("/tmp/unused", &[("OSS_SEARCH_SLOTS", "64")]).is_err());
    let ok = config("/tmp/unused", &[("OSS_SEARCH_SLOTS", "63")]).expect("valid");
    assert_eq!(ok.search_slots.slots, 63);
    // The checks are skipped with the gate off, whatever the other values.
    assert!(config("/tmp/unused", &[("OSS_SEARCH_MAX_WAITERS_PCT", "100")]).is_ok());
}

// AC-11
#[test]
fn a_node_with_slots_serves_searches_and_reports_the_gate() {
    let dir = TempDir::new("boot-search-slots");
    let node = boot(dir.path(), &[("OSS_SEARCH_SLOTS", "8")]);
    assert_eq!(call(&node, "PUT", "/logs", None).status, 200);
    let put = call(
        &node,
        "PUT",
        "/logs/_doc/1?refresh=true",
        Some(r#"{"n":1}"#),
    );
    assert_eq!(put.status, 201, "{}", put.text());
    let found = call(
        &node,
        "POST",
        "/logs/_search",
        Some(r#"{"query":{"match_all":{}}}"#),
    );
    assert_eq!(found.status, 200, "{}", found.text());
    let usage = call(&node, "GET", "/_serverless/usage", None).text();
    assert!(
        usage.contains("\"search_slots\":{\"slots\":8,\"in_use\":0,\"waiting\":0,\"admitted\":1,"),
        "{usage}"
    );
    let dir = TempDir::new("boot-search-slots-off");
    let node = boot(dir.path(), &[]);
    let usage = call(&node, "GET", "/_serverless/usage", None).text();
    assert!(!usage.contains("search_slots"), "{usage}");
}
