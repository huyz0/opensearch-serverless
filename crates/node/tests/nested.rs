//! docs/specs/nested-fields: a nested mapping written and searched through a
//! node, with the switch on and off.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use suite_rig::{Cluster, wait_for};

const MAPPING: &str = r#"{"settings":{"index":{"number_of_shards":1}},"mappings":{"properties":{"title":{"type":"keyword"},"comments":{"type":"nested","properties":{"who":{"type":"keyword"},"n":{"type":"long"}}}}}}"#;

fn bulk(cluster: &Cluster, lines: &[&str], refresh: bool) {
    let mut body = lines.join("\n");
    body.push('\n');
    let target = if refresh {
        "/_bulk?refresh=true"
    } else {
        "/_bulk"
    };
    let r = cluster.call(0, "POST", target, Some(&body));
    assert!(r.text().contains("\"errors\":false"), "{}", r.text());
}

fn total(cluster: &Cluster, query: &str) -> Option<String> {
    let body = format!(r#"{{"size":0,"track_total_hits":true,"query":{query}}}"#);
    let r = cluster.call(0, "POST", "/blog/_search", Some(&body));
    (r.status == 200).then(|| r.text())
}

fn has_total(cluster: &Cluster, query: &str, n: u32) -> bool {
    total(cluster, query).is_some_and(|t| t.contains(&format!("\"value\":{n}")))
}

const BOTH: &str = r#"{"nested":{"path":"comments","query":{"term":{"comments.who":"ann"}}}}"#;

// docs/specs/nested-fields AC-9
#[test]
fn a_nested_index_is_written_searched_updated_and_deleted_through_a_node() {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    cluster.set("OSS_NESTED", "true");
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    let made = cluster.call(0, "PUT", "/blog", Some(MAPPING));
    assert_eq!(made.status, 200, "{}", made.text());
    let a = r#"{"title":"a","comments":[{"who":"ann","n":1},{"who":"bob","n":2}]}"#;
    let b = r#"{"title":"b","comments":[{"who":"bob","n":3}]}"#;
    // Only in the log: the answers hold before a publish.
    bulk(
        &cluster,
        &[
            r#"{"index":{"_index":"blog","_id":"1"}}"#,
            a,
            r#"{"index":{"_index":"blog","_id":"2"}}"#,
            b,
        ],
        true,
    );
    wait_for("the nested query finds the parent", 40, || {
        has_total(&cluster, BOTH, 1)
    });
    wait_for("the total counts parents", 40, || {
        has_total(&cluster, r#"{"match_all":{}}"#, 2)
    });
    // An update replaces the children.
    let a2 = r#"{"title":"a","comments":[{"who":"cat","n":9}]}"#;
    bulk(
        &cluster,
        &[r#"{"index":{"_index":"blog","_id":"1"}}"#, a2],
        true,
    );
    wait_for("the old children are gone", 40, || {
        has_total(&cluster, BOTH, 0)
    });
    assert!(has_total(
        &cluster,
        r#"{"nested":{"path":"comments","query":{"term":{"comments.who":"cat"}}}}"#,
        1
    ));
    // A delete removes the block.
    bulk(
        &cluster,
        &[r#"{"delete":{"_index":"blog","_id":"1"}}"#],
        true,
    );
    wait_for("the deleted parent and its children are gone", 40, || {
        has_total(&cluster, r#"{"match_all":{}}"#, 1)
            && has_total(
                &cluster,
                r#"{"nested":{"path":"comments","query":{"term":{"comments.who":"cat"}}}}"#,
                0,
            )
    });
    // A get returns the source.
    let got = cluster.call(0, "GET", "/blog/_doc/2", None);
    assert_eq!(got.status, 200, "{}", got.text());
    assert!(got.text().contains(r#""who":"bob""#), "{}", got.text());
}

// docs/specs/nested-fields AC-10
#[test]
fn a_nested_mapping_is_refused_with_the_switch_off() {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    cluster.set("OSS_NESTED", "false");
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    let made = cluster.call(0, "PUT", "/blog", Some(MAPPING));
    assert_eq!(made.status, 400, "{}", made.text());
    assert!(
        made.text().contains("field type [nested]"),
        "{}",
        made.text()
    );
}

// docs/specs/nested-fields AC-10
#[test]
fn an_index_made_with_the_switch_on_still_reads_and_writes_with_it_off() {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    cluster.set("OSS_NESTED", "true");
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    let made = cluster.call(0, "PUT", "/blog", Some(MAPPING));
    assert_eq!(made.status, 200, "{}", made.text());
    bulk(
        &cluster,
        &[
            r#"{"index":{"_index":"blog","_id":"1"}}"#,
            r#"{"title":"a","comments":[{"who":"ann","n":1}]}"#,
        ],
        true,
    );
    cluster.retire(0);
    cluster.set("OSS_NESTED", "false");
    let slot = cluster.add("n2");
    assert_eq!(slot, 1);
    wait_for("the new node sees itself", 30, || {
        cluster.nodes_seen(1) == 1
    });
    let found = |q: &str| {
        let body = format!(r#"{{"size":0,"track_total_hits":true,"query":{q}}}"#);
        let r = cluster.call(1, "POST", "/blog/_search", Some(&body));
        r.status == 200 && r.text().contains("\"value\":1")
    };
    wait_for("the nested query still finds the parent", 60, || {
        found(BOTH)
    });
    let body = "{\"index\":{\"_index\":\"blog\",\"_id\":\"2\"}}\n{\"title\":\"b\",\"comments\":[{\"who\":\"ann\",\"n\":2}]}\n";
    let r = cluster.call(1, "POST", "/_bulk?refresh=true", Some(body));
    assert!(r.text().contains("\"errors\":false"), "{}", r.text());
}

// docs/specs/nested-fields AC-9
#[test]
fn a_nested_document_in_the_log_is_searched_beside_a_published_one() {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    cluster.set("OSS_NESTED", "true");
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    let made = cluster.call(0, "PUT", "/blog", Some(MAPPING));
    assert_eq!(made.status, 200, "{}", made.text());
    bulk(
        &cluster,
        &[
            r#"{"index":{"_index":"blog","_id":"1"}}"#,
            r#"{"title":"a","comments":[{"who":"ann","n":1},{"who":"bob","n":2}]}"#,
        ],
        true,
    );
    // Retiring publishes document 1; the next node starts from it.
    cluster.retire(0);
    let slot = cluster.add("n2");
    wait_for("the new node sees itself", 30, || {
        cluster.nodes_seen(slot) == 1
    });
    let on = |q: &str, n: u32| {
        let body = format!(r#"{{"size":0,"track_total_hits":true,"query":{q}}}"#);
        let r = cluster.call(slot, "POST", "/blog/_search", Some(&body));
        r.status == 200 && r.text().contains(&format!("\"value\":{n}"))
    };
    wait_for("the published document is found", 60, || on(BOTH, 1));
    // Document 2 is only in the log, beside the published one.
    let body = "{\"index\":{\"_index\":\"blog\",\"_id\":\"2\"}}\n{\"title\":\"b\",\"comments\":[{\"who\":\"ann\",\"n\":3}]}\n";
    let r = cluster.call(slot, "POST", "/_bulk?refresh=true", Some(body));
    assert!(r.text().contains("\"errors\":false"), "{}", r.text());
    wait_for("both documents match the nested query", 60, || on(BOTH, 2));
    wait_for("the total counts two parents", 60, || {
        on(r#"{"match_all":{}}"#, 2)
    });
    let agg = r#"{"size":0,"aggs":{"c":{"nested":{"path":"comments"},"aggs":{"p":{"reverse_nested":{}}}}}}"#;
    let r = cluster.call(slot, "POST", "/blog/_search", Some(agg));
    assert_eq!(r.status, 200, "{}", r.text());
    let text = r.text();
    // Three children across the two parents, which step back out to two.
    assert!(text.contains("\"doc_count\":3"), "{text}");
    assert!(text.contains("\"doc_count\":2"), "{text}");
}
