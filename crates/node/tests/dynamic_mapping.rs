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

/// Whether the mapping the node in `slot` reads back holds field `n`.
fn mapped(cluster: &Cluster, slot: usize) -> bool {
    let r = cluster.call(slot, "GET", "/dyn/_mapping", None);
    r.status == 200 && r.text().contains(r#""n":{"type":"long"}"#)
}

// docs/specs/node-index-mapping-get AC-4; docs/specs/dynamic-mapping-durable AC-2
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
        wait_for("the mapping holds the field", 40, || mapped(&cluster, slot));
    }
}

fn bulk_one(cluster: &Cluster, id: &str, doc: &str) -> bool {
    let body = format!("{{\"index\":{{\"_index\":\"upd\",\"_id\":\"{id}\"}}}}\n{doc}\n");
    let r = cluster.call(0, "POST", "/_bulk?refresh=true", Some(&body));
    r.status == 200 && r.text().contains("\"errors\":false")
}

// docs/specs/node-index-mapping-put AC-9
#[test]
fn a_write_after_a_mapping_update_is_indexed_by_the_updated_type() {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    let made = cluster.call(
        0,
        "PUT",
        "/upd",
        Some(r#"{"settings":{"index":{"number_of_shards":1}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    // The shard opens with the first write, before the update.
    assert!(bulk_one(&cluster, "1", r#"{"n":1}"#));
    let put = cluster.call(
        0,
        "PUT",
        "/upd/_mapping",
        Some(r#"{"properties":{"s":{"type":"keyword"}}}"#),
    );
    assert_eq!(put.status, 200, "{}", put.text());
    // The held shard may refuse the first write that meets the update and
    // reopen from the descriptor; the write goes through when sent again.
    wait_for("a write of the updated field is acknowledged", 20, || {
        bulk_one(&cluster, "2", r#"{"s":"hello world"}"#)
    });
    let term = r#"{"size":0,"track_total_hits":true,"query":{"term":{"s":"hello world"}}}"#;
    wait_for("the keyword is found whole", 40, || {
        let r = cluster.call(0, "POST", "/upd/_search", Some(term));
        r.status == 200 && r.text().contains("\"value\":1")
    });
}
