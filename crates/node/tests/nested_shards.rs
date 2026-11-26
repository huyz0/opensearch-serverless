//! docs/specs/nested-fields: a nested index spread over several shards, searched
//! through one node so the coordinator merges the shards' answers.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use suite_rig::{Cluster, wait_for};

const SHARDS: u32 = 4;
const DOCS: u32 = 12;

fn search(cluster: &Cluster, body: &str) -> Option<String> {
    let r = cluster.call(0, "POST", "/blog/_search", Some(body));
    (r.status == 200).then(|| r.text())
}

// docs/specs/nested-fields AC-9
#[test]
fn a_nested_index_over_several_shards_merges_hits_sorts_and_aggregations() {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    cluster.set("OSS_NESTED", "true");
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    let mapping = format!(
        r#"{{"settings":{{"index":{{"number_of_shards":{SHARDS}}}}},"mappings":{{"properties":{{"title":{{"type":"keyword"}},"comments":{{"type":"nested","properties":{{"who":{{"type":"keyword"}},"n":{{"type":"long"}}}}}}}}}}}}"#
    );
    let made = cluster.call(0, "PUT", "/blog", Some(&mapping));
    assert_eq!(made.status, 200, "{}", made.text());
    // Document i has i comments: ann on every one, bob on the even ones.
    let mut lines = Vec::new();
    for i in 1..=DOCS {
        lines.push(format!(r#"{{"index":{{"_index":"blog","_id":"{i}"}}}}"#));
        let comments: Vec<String> = (0..i)
            .map(|k| {
                let who = if k == 0 || i % 2 == 1 { "ann" } else { "bob" };
                format!(r#"{{"who":"{who}","n":{i}}}"#)
            })
            .collect();
        lines.push(format!(
            r#"{{"title":"t{i:02}","comments":[{}]}}"#,
            comments.join(",")
        ));
    }
    let body = lines.join("\n") + "\n";
    let r = cluster.call(0, "POST", "/_bulk?refresh=true", Some(&body));
    assert!(r.text().contains("\"errors\":false"), "{}", r.text());

    let ann = r#"{"nested":{"path":"comments","query":{"term":{"comments.who":"ann"}}}}"#;
    let count = format!(r#"{{"size":0,"track_total_hits":true,"query":{ann}}}"#);
    wait_for("every shard's parents are counted", 60, || {
        search(&cluster, &count).is_some_and(|t| t.contains(&format!("\"value\":{DOCS}")))
    });
    // Parents holding bob: the even ones.
    let bob = r#"{"nested":{"path":"comments","query":{"term":{"comments.who":"bob"}}}}"#;
    let text = search(
        &cluster,
        &format!(r#"{{"size":0,"track_total_hits":true,"query":{bob}}}"#),
    )
    .expect("answers");
    assert!(text.contains("\"value\":6"), "{text}");
    // The merged page orders by a parent field across shards.
    let page =
        format!(r#"{{"size":3,"sort":[{{"title":"desc"}}],"query":{ann},"_source":["title"]}}"#);
    let text = search(&cluster, &page).expect("answers");
    let at = |t: &str| text.find(t).unwrap_or_else(|| panic!("{t} in {text}"));
    assert!(at("t12") < at("t11") && at("t11") < at("t10"), "{text}");
    assert!(!text.contains("t09"), "{text}");
    assert!(
        text.contains(r#""total":4"#),
        "four shards answered: {text}"
    );
    // Nested sort and inner hits merge across shards too.
    let by_child = format!(
        r#"{{"size":2,"query":{ann},"_source":false,"sort":[{{"comments.n":{{"order":"desc","nested":{{"path":"comments"}}}}}}]}}"#
    );
    let text = search(&cluster, &by_child).expect("nested sort answers");
    assert!(
        text.contains(r#""_id":"12""#) && text.contains(r#""_id":"11""#),
        "{text}"
    );
    let inner = r#"{"size":12,"query":{"nested":{"path":"comments","query":{"term":{"comments.who":"bob"}},"inner_hits":{"size":20}}},"_source":false}"#;
    let text = search(&cluster, inner).expect("inner hits answer");
    assert_eq!(
        text.matches(r#""_nested":{"field":"comments""#).count(),
        36,
        "{text}"
    );
    // Children across shards: 1 + 2 + ... + 12, stepping back out to 12 parents.
    let aggs = r#"{"size":0,"aggs":{"c":{"nested":{"path":"comments"},"aggs":{
        "total":{"sum":{"field":"comments.n"}},
        "who":{"terms":{"field":"comments.who"},"aggs":{"up":{"reverse_nested":{}}}},
        "up":{"reverse_nested":{}}}}}}"#;
    let text = search(&cluster, aggs).expect("answers");
    assert!(text.contains(r#""doc_count":78"#), "{text}");
    assert!(text.contains(r#""up":{"doc_count":12}"#), "{text}");
    // Sum of i * i for i in 1..=12.
    assert!(text.contains(r#""total":{"value":650.0}"#), "{text}");
    // ann: every comment of an odd parent (36) and the first of each even one (6); bob the rest (36).
    assert!(
        text.contains(r#"{"key":"ann","doc_count":42,"up":{"doc_count":12}}"#),
        "{text}"
    );
    assert!(
        text.contains(r#"{"key":"bob","doc_count":36,"up":{"doc_count":6}}"#),
        "{text}"
    );
}
