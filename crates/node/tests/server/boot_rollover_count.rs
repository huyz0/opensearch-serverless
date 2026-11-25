//! docs/specs/rollover-count AC-1: a booted node's rollover decides
//! `max_docs` by the documents the index holds.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

use crate::support;

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn boot(root: &std::path::Path, templates: bool) -> Node {
    let root = root.display().to_string();
    let config = NodeConfig::from_lookup(|key| {
        Some(
            match key {
                "OSS_NODE_ID" => "n1",
                "OSS_STORE" => "fs",
                "OSS_FS_ROOT" => root.as_str(),
                "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                "OSS_INDEX_TEMPLATES" if templates => "true",
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

// AC-1
#[test]
fn a_booted_node_rolls_an_alias_over_once_its_index_holds_enough_documents() {
    let dir = TempDir::new("boot-rollover-count");
    let node = boot(dir.path(), false);
    assert_eq!(call(&node, "PUT", "/idx-000001", None).status, 200);
    let alias = call(
        &node,
        "PUT",
        "/idx-000001/_alias/w",
        Some(r#"{"is_write_index":true}"#),
    );
    assert_eq!(alias.status, 200, "{}", alias.text());
    let roll = |max: u32| {
        let body = format!(r#"{{"conditions":{{"max_docs":{max}}}}}"#);
        let reply = call(&node, "POST", "/w/_rollover", Some(&body));
        assert_eq!(reply.status, 200, "{}", reply.text());
        serde_json::from_str::<serde_json::Value>(&reply.text()).expect("json")
    };
    for id in 1..=3 {
        let put = call(
            &node,
            "PUT",
            &format!("/idx-000001/_doc/{id}?refresh=true"),
            Some(r#"{"n":1}"#),
        );
        assert_eq!(put.status, 201, "{}", put.text());
    }
    let few = roll(4);
    assert_eq!(few["rolled_over"], false, "{few}");
    assert_eq!(few["conditions"]["[max_docs: 4]"], false);
    let enough = roll(3);
    assert_eq!(enough["rolled_over"], true, "{enough}");
    assert_eq!(enough["new_index"], "idx-000002");
    // The new index holds nothing yet, so one document is too many to ask.
    let fresh = roll(1);
    assert_eq!(fresh["old_index"], "idx-000002");
    assert_eq!(fresh["rolled_over"], false, "{fresh}");
}

// AC-2
#[test]
fn a_booted_node_makes_rolls_and_removes_a_stream_inside_its_default_budget() {
    let dir = TempDir::new("boot-stream-default-budget");
    let node = boot(dir.path(), true);
    let template = r#"{"index_patterns":["logs-*"],"data_stream":{},"template":{}}"#;
    for (method, target, body, status) in [
        ("PUT", "/_index_template/logs", Some(template), 200),
        ("PUT", "/_data_stream/logs-a", None, 200),
        ("POST", "/logs-a/_rollover", None, 200),
        (
            "POST",
            "/logs-a/_rollover",
            Some(r#"{"conditions":{"max_docs":1}}"#),
            200,
        ),
        ("DELETE", "/_data_stream/logs-a", None, 200),
    ] {
        let reply = call(&node, method, target, body);
        assert_eq!(reply.status, status, "{method} {target}: {}", reply.text());
    }
}
