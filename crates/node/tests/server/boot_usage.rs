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
        "OSS_TENANT_TICK_MS" => Some("20".to_owned()),
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
    let node = boot(dir.path(), Some("false"));
    let reply = call(&node, "GET", "/_serverless/usage", None);
    assert_ne!(reply.status, 200, "{}", reply.text());
    assert!(!reply.text().contains("cost_units"));
}

// AC-3
#[test]
fn a_node_with_no_store_usage_setting_serves_the_usage_route() {
    let dir = TempDir::new("boot-usage-default");
    let node = boot(dir.path(), None);
    let reply = call(&node, "GET", "/_serverless/usage", None);
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(reply.text().contains("cost_units"), "{}", reply.text());
}

// docs/specs/tenant-levels-wiring AC-8
#[test]
fn an_index_created_with_a_bucket_group_bills_to_that_tenant() {
    let dir = TempDir::new("boot-tenants");
    let node = boot(dir.path(), Some("true"));
    let made = call(
        &node,
        "PUT",
        "/logs-a",
        Some(r#"{"settings":{"index.serverless.bucket_group":"acme"}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    assert_eq!(call(&node, "PUT", "/plain", None).status, 200);
    for index in ["logs-a", "plain"] {
        let put = call(
            &node,
            "PUT",
            &format!("/{index}/_doc/1"),
            Some(r#"{"n":1}"#),
        );
        assert_eq!(put.status, 201, "{}", put.text());
    }
    let started = std::time::Instant::now();
    loop {
        let body = call(&node, "GET", "/_serverless/usage", None).text();
        if body.contains("\"acme\":{\"level\"") && body.contains("\"plain\":{\"level\"") {
            assert!(!body.contains("\"tenants\":{\"logs-a\""), "{body}");
            assert!(!body.contains("\"logs-a\":{\"level\""), "{body}");
            return;
        }
        assert!(started.elapsed().as_secs() < 10, "{body}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// A module that follows the ABI and answers every call with the three bytes
/// `abc`.
#[cfg(feature = "extensions")]
fn answering_module() -> Vec<u8> {
    const DESCRIPTOR: &str = r#"{"name":"usage_test","version":"1.0.0","abi":"1.0","io_bytes":1024,"max_concurrent_calls":4,"points":[{"id":"echo","version":"1.0.0"}]}"#;
    let escaped: String = DESCRIPTOR.bytes().map(|b| format!("\\{b:02x}")).collect();
    wat::parse_str(format!(
        r#"(module
  (memory (export "memory") 4)
  (data (i32.const 0) "{escaped}")
  (data (i32.const 1000) "abc")
  (func (export "oss_abi_version") (result i32) (i32.const 65536))
  (func (export "oss_describe") (result i32)
    (memory.copy (i32.const 131072) (i32.const 0) (i32.const {len})) (i32.const {len}))
  (func (export "oss_input_region") (result i32) (i32.const 65536))
  (func (export "oss_output_region") (result i32) (i32.const 131072))
  (func (export "oss_result_len") (result i32) (i32.const 3))
  (func (export "oss_call") (param i32 i32 i32) (result i32)
    (memory.copy (i32.const 131072) (i32.const 1000) (i32.const 3)) (i32.const 0)))"#,
        len = DESCRIPTOR.len()
    ))
    .expect("valid module")
}

// docs/specs/extension-analyzer AC-17: the counters are on the usage route
#[cfg(feature = "extensions")]
#[test]
fn a_tenants_module_calls_appear_on_the_usage_route_of_a_running_node() {
    let dir = TempDir::new("boot-usage-extensions");
    let node = boot(dir.path(), Some("true"));
    let before = call(&node, "GET", "/_serverless/usage", None).text();
    assert!(
        !before.contains("usage-route-tenant"),
        "no calls yet for this tenant: {before}"
    );
    let module = oss_node::extension_host::host()
        .load(
            &answering_module(),
            std::sync::Arc::new(oss_ext_wasm::Unbounded),
        )
        .expect("loads");
    for _ in 0..3 {
        assert_eq!(
            module
                .call("usage-route-tenant", 0, b"hello")
                .expect("call"),
            b"abc"
        );
    }
    let body = call(&node, "GET", "/_serverless/usage", None).text();
    let at = body
        .find("\"usage-route-tenant\":{\"calls\":3,")
        .unwrap_or_else(|| panic!("the tenant's counters are in the body: {body}"));
    let counters = &body[at..];
    assert!(
        counters.contains("\"input_bytes\":15,\"output_bytes\":9}"),
        "{counters}"
    );
}
