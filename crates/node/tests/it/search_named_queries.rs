//! docs/specs/named-queries: `_name` and `matched_queries` through HTTP.

use crate::support::{request, start};

fn post(body: &str) -> (u16, String) {
    let node = start();
    let r = request(
        node.addr(),
        "POST",
        "/compat-paging-sort/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

/// Each hit's `_id` with what `matched_queries` holds for it.
fn matched(text: &str) -> Vec<(String, serde_json::Value)> {
    let v: serde_json::Value = serde_json::from_str(text).expect("json");
    v["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| {
            (
                h["_id"].as_str().expect("id").to_owned(),
                h.get("matched_queries")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            )
        })
        .collect()
}

const BODY: &str = r#"{"query":{"bool":{
    "must":[{"match_all":{"_name":"all"}}],
    "should":[
        {"term":{"tag":{"value":"animal","_name":"animal"}}},
        {"range":{"year":{"gte":2002,"_name":"late"}}},
        {"match":{"title":{"query":"quick","_name":"quick"}}}
    ],
    "filter":[{"exists":{"field":"title","_name":"titled"}}],
    "must_not":[{"term":{"tag":{"value":"food","_name":"never"}}}],
    "_name":"top"
}},"size":10,"sort":[{"year":"asc"}]}"#;

#[test]
fn a_hit_lists_the_named_queries_that_match_it() {
    let (status, text) = post(BODY);
    assert_eq!(status, 200, "{text}");
    let hits = matched(&text);
    assert!(!hits.is_empty(), "{text}");
    for (id, names) in &hits {
        let names: Vec<&str> = names
            .as_array()
            .unwrap_or_else(|| panic!("hit {id} has no matched_queries: {text}"))
            .iter()
            .map(|n| n.as_str().expect("name"))
            .collect();
        // Named queries a hit cannot fail: the match_all, the filter, the bool.
        for always in ["all", "titled", "top"] {
            assert!(names.contains(&always), "{id}: {names:?}");
        }
        // A must_not that matched would have dropped the hit.
        assert!(!names.contains(&"never"), "{id}: {names:?}");
    }
    // The manual (2010, "Quick start guide") matches `late` and `quick`, not `animal`.
    let manual = hits
        .iter()
        .find(|(_, n)| n.to_string().contains("quick") && !n.to_string().contains("animal"))
        .expect("a hit matching quick and not animal");
    assert!(manual.1.to_string().contains("late"), "{:?}", manual.1);
}

#[test]
fn include_named_queries_score_writes_an_object_of_scores() {
    let body = BODY.replacen(
        "\"size\"",
        "\"include_named_queries_score\":true,\"size\"",
        1,
    );
    let (status, text) = post(&body);
    assert_eq!(status, 200, "{text}");
    for (id, names) in matched(&text) {
        let object = names
            .as_object()
            .unwrap_or_else(|| panic!("{id}: {names} is not an object"));
        assert!(
            object["all"].as_f64().expect("score") > 0.0,
            "{id}: {names}"
        );
    }
}

#[test]
fn naming_a_query_changes_neither_hits_nor_scores() {
    let named =
        r#"{"query":{"term":{"tag":{"value":"animal","_name":"n"}}},"sort":[{"year":"asc"}]}"#;
    let bare = r#"{"query":{"term":{"tag":{"value":"animal"}}},"sort":[{"year":"asc"}]}"#;
    let (s1, a) = post(named);
    let (s2, b) = post(bare);
    assert_eq!((s1, s2), (200, 200), "{a}{b}");
    let ids = |t: &str| -> Vec<String> { matched(t).into_iter().map(|(i, _)| i).collect() };
    assert_eq!(ids(&a), ids(&b));
    assert!(matched(&b).iter().all(|(_, m)| m.is_null()), "{b}");
    assert!(
        matched(&a)
            .iter()
            .all(|(_, m)| *m == serde_json::json!(["n"])),
        "{a}"
    );
}

#[test]
fn a_name_that_is_not_a_string_and_too_many_names_are_refused() {
    let (status, text) = post(r#"{"query":{"term":{"tag":{"value":"animal","_name":5}}}}"#);
    assert_eq!(status, 400, "{text}");
    let clauses: Vec<String> = (0..65)
        .map(|i| format!(r#"{{"match_all":{{"_name":"n{i}"}}}}"#))
        .collect();
    let body = format!(
        r#"{{"query":{{"bool":{{"should":[{}]}}}}}}"#,
        clauses.join(",")
    );
    let (status, text) = post(&body);
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("too many named queries"), "{text}");
}
