//! docs/specs/nested-aggs: `nested` and `reverse_nested` aggregations
//! answered through a node.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use suite_rig::{Cluster, wait_for};

const MAPPING: &str = r#"{"settings":{"index":{"number_of_shards":1}},"mappings":{"properties":{"price":{"type":"long"},"meta":{"properties":{"k":{"type":"keyword"}}},"comments":{"type":"nested","properties":{"who":{"type":"keyword"},"n":{"type":"long"}}}}}}"#;

fn aggregate(cluster: &Cluster, aggs: &str) -> Option<String> {
    let body = format!(r#"{{"size":0,"aggs":{aggs}}}"#);
    let r = cluster.call(0, "POST", "/shop/_search", Some(&body));
    (r.status == 200).then(|| r.text())
}

// docs/specs/nested-aggs AC-8
#[test]
fn nested_and_reverse_nested_aggregate_through_a_node() {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    cluster.set("OSS_NESTED", "true");
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    let made = cluster.call(0, "PUT", "/shop", Some(MAPPING));
    assert_eq!(made.status, 200, "{}", made.text());
    let body = [
        r#"{"index":{"_index":"shop","_id":"1"}}"#,
        r#"{"price":10,"comments":[{"who":"ann","n":1},{"who":"bob","n":2}]}"#,
        r#"{"index":{"_index":"shop","_id":"2"}}"#,
        r#"{"price":20,"comments":[{"who":"bob","n":3}]}"#,
    ]
    .join("\n")
        + "\n";
    let r = cluster.call(0, "POST", "/_bulk?refresh=true", Some(&body));
    assert!(r.text().contains("\"errors\":false"), "{}", r.text());
    let aggs = r#"{"c":{"nested":{"path":"comments"},"aggs":{
        "who":{"terms":{"field":"comments.who"}},
        "total":{"sum":{"field":"comments.n"}},
        "up":{"reverse_nested":{},"aggs":{"price":{"sum":{"field":"price"}}}}}}}"#;
    wait_for("the nested aggregation answers", 40, || {
        aggregate(&cluster, aggs).is_some_and(|t| t.contains(r#""doc_count":3"#))
    });
    let text = aggregate(&cluster, aggs).expect("answers");
    assert!(text.contains(r#""total":{"value":6.0}"#), "{text}");
    assert!(
        text.contains(r#""up":{"doc_count":2,"price":{"value":30.0}}"#),
        "{text}"
    );
    assert!(text.contains(r#"{"key":"bob","doc_count":2}"#), "{text}");
    // The parents holding each commenter, through a `reverse_nested` under a `terms`.
    let who = r#"{"c":{"nested":{"path":"comments"},"aggs":{"who":{"terms":{"field":"comments.who"},
        "aggs":{"up":{"reverse_nested":{}}}}}}}"#;
    let text = aggregate(&cluster, who).expect("answers");
    assert!(
        text.contains(r#"{"key":"bob","doc_count":2,"up":{"doc_count":2}}"#),
        "{text}"
    );
    assert!(
        text.contains(r#"{"key":"ann","doc_count":1,"up":{"doc_count":1}}"#),
        "{text}"
    );
    // A path that is an object but not nested is refused.
    let body = r#"{"size":0,"aggs":{"c":{"nested":{"path":"meta"}}}}"#;
    let refused = cluster.call(0, "POST", "/shop/_search", Some(body));
    assert_eq!(refused.status, 500, "{}", refused.text());
    assert!(
        refused
            .text()
            .contains("[nested] nested path [meta] is not nested"),
        "{}",
        refused.text()
    );
    // A path that holds no nested object counts nothing.
    let none = aggregate(&cluster, r#"{"c":{"nested":{"path":"nope"}}}"#).expect("answers");
    assert!(none.contains(r#""c":{"doc_count":0}"#), "{none}");
}
