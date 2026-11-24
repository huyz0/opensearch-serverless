//! docs/specs/boot-templates-ingest AC-1: which template and pipeline routes
//! a booted node serves.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

use crate::support;

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn boot(root: &std::path::Path, set: &[&str]) -> Node {
    let root = root.display().to_string();
    let config = NodeConfig::from_lookup(|key| {
        Some(
            match key {
                "OSS_NODE_ID" => "n1",
                "OSS_STORE" => "fs",
                "OSS_FS_ROOT" => root.as_str(),
                "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                other if set.contains(&other) => "true",
                _ => return None,
            }
            .to_owned(),
        )
    })
    .expect("valid");
    start(&config).expect("starts")
}

fn status(node: &Node, method: &str, target: &str, body: Option<&str>) -> u16 {
    support::request(node.addr(), method, target, body.map(str::as_bytes)).status
}

const TEMPLATE: &str = r#"{"index_patterns":["logs-*"],"template":{"mappings":{"properties":{"n":{"type":"integer"}}}}}"#;
const PIPELINE: &str = r#"{"processors":[{"set":{"field":"a","value":"b"}}]}"#;

#[test]
fn a_booted_node_serves_the_template_and_pipeline_routes_only_when_each_is_set() {
    let off = TempDir::new("boot-authoring-off");
    let node = boot(off.path(), &[]);
    let put_template = |n: &Node| status(n, "PUT", "/_index_template/t", Some(TEMPLATE));
    let put_pipeline = |n: &Node| status(n, "PUT", "/_ingest/pipeline/p", Some(PIPELINE));
    let (template_off, pipeline_off) = (put_template(&node), put_pipeline(&node));
    assert!(template_off >= 400, "templates off: {template_off}");
    assert!(pipeline_off >= 400, "pipelines off: {pipeline_off}");

    let templates = TempDir::new("boot-authoring-templates");
    let node = boot(templates.path(), &["OSS_INDEX_TEMPLATES"]);
    assert_eq!(put_template(&node), 200);
    assert_eq!(put_pipeline(&node), pipeline_off);

    let pipelines = TempDir::new("boot-authoring-pipelines");
    let node = boot(pipelines.path(), &["OSS_INGEST_PIPELINES"]);
    assert_eq!(put_pipeline(&node), 200);
    assert_eq!(put_template(&node), template_off);
}
