//! docs/specs/dynamic-mapping-durable: a field a write introduces is held
//! in the descriptor, so every node maps it.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use suite_rig::{Cluster, wait_for};

const DOCS: usize = 20;

fn found(cluster: &Cluster, slot: usize) -> bool {
    let body = r#"{"size":0,"track_total_hits":true,"query":{"range":{"n":{"gte":0}}}}"#;
    let r = cluster.call(slot, "POST", "/dyn/_search", Some(body));
    r.status == 200 && r.text().contains(&format!("\"value\":{DOCS}"))
}

// AC-2
#[test]
fn a_range_query_on_a_dynamically_mapped_field_finds_every_document_on_every_node() {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    for id in ["n1", "n2"] {
        cluster.add(id);
    }
    wait_for("both nodes see two", 30, || {
        (0..2).all(|s| cluster.nodes_seen(s) == 2)
    });
    let made = cluster.call(
        0,
        "PUT",
        "/dyn",
        Some(r#"{"settings":{"index":{"number_of_shards":1}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    let mut body = String::new();
    for i in 0..DOCS {
        body.push_str(&format!(
            "{{\"index\":{{\"_index\":\"dyn\",\"_id\":\"{i}\"}}}}\n{{\"n\":{i}}}\n"
        ));
    }
    let r = cluster.call(0, "POST", "/_bulk?refresh=true", Some(&body));
    assert!(r.text().contains("\"errors\":false"), "{}", r.text());
    for slot in 0..2 {
        wait_for("the range query finds every document", 40, || {
            found(&cluster, slot)
        });
    }
}
