//! docs/specs/search-routing: `routing` narrows the shards a search visits
//! and `preference` is accepted.

use oss_cluster::routing::DocumentRouting;

use crate::support::{Node, put_shard_copy, put_variant, request, start};

const UUID: &str = "compat-term-uuid";
const TERM: &str = r#"{"query":{"term":{"title":"brown"}}}"#;

/// Two shards holding copies of the same two matching documents, so a total
/// of 4 means both were visited and 2 means one.
fn two_shards() -> Node {
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", UUID, 1);
    node
}

/// A routing value that routes to `shard` of two.
fn routing_to(shard: u32) -> String {
    let routing = DocumentRouting::legacy_scaled(2, 1, 2).expect("layout");
    (0..64)
        .map(|n| format!("r{n}"))
        .find(|r| routing.shard_for(r, Some(r)) == Ok(shard))
        .expect("a value for the shard")
}

fn total(node: &Node, path: &str) -> (u64, String) {
    let r = request(node.addr(), "POST", path, Some(TERM.as_bytes()));
    assert_eq!(r.status, 200, "{}", r.text());
    let text = r.text();
    let from = text.find(r#""total":{"value":"#).expect("total") + 17;
    let n = text[from..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|d| d.parse().ok())
        .expect("a number");
    (n, text)
}

#[test]
fn a_routing_value_visits_the_shard_it_routes_to() {
    let node = two_shards();
    assert_eq!(total(&node, "/two/_search").0, 4);
    for shard in [0, 1] {
        let (n, text) = total(
            &node,
            &format!("/two/_search?routing={}", routing_to(shard)),
        );
        assert_eq!(n, 2, "{text}");
        assert!(
            text.contains(&format!("[two][{shard}]")) || !text.contains("[two]["),
            "{text}"
        );
    }
}

#[test]
fn several_values_visit_each_shard_and_an_empty_one_visits_all() {
    let node = two_shards();
    let both = format!("{},{}", routing_to(0), routing_to(1));
    assert_eq!(total(&node, &format!("/two/_search?routing={both}")).0, 4);
    assert_eq!(total(&node, "/two/_search?routing=").0, 4);
    let same = format!("/compat-term/_search?routing={}", routing_to(1));
    assert_eq!(
        total(&node, &same).0,
        total(&node, "/compat-term/_search").0
    );
}

#[test]
fn preference_is_accepted_and_changes_nothing() {
    let node = two_shards();
    assert_eq!(total(&node, "/two/_search?preference=_local").0, 4);
    assert_eq!(total(&node, "/two/_search?preference=whatever").0, 4);
}

#[test]
fn an_msearch_header_routes_and_prefers() {
    let node = two_shards();
    let header = format!(
        r#"{{"index":"two","routing":"{}","preference":"_local"}}"#,
        routing_to(1)
    );
    let body = format!("{header}\n{TERM}\n{{\"index\":\"two\"}}\n{TERM}\n");
    let r = request(node.addr(), "POST", "/_msearch", Some(body.as_bytes()));
    assert_eq!(r.status, 200, "{}", r.text());
    let text = r.text();
    assert_eq!(text.matches(r#""total":{"value":2"#).count(), 1, "{text}");
    assert_eq!(text.matches(r#""total":{"value":4"#).count(), 1, "{text}");
}

fn count(node: &Node, path: &str) -> u64 {
    let r = request(
        node.addr(),
        "POST",
        path,
        Some(br#"{"query":{"term":{"title":"brown"}}}"#),
    );
    assert_eq!(r.status, 200, "{}", r.text());
    let text = r.text();
    let from = text.find(r#""count":"#).expect("count") + 8;
    text[from..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|d| d.parse().ok())
        .expect("a number")
}

#[test]
fn a_count_visits_the_shards_its_routing_names() {
    let node = two_shards();
    assert_eq!(count(&node, "/two/_count"), 4);
    for shard in [0, 1] {
        assert_eq!(
            count(&node, &format!("/two/_count?routing={}", routing_to(shard))),
            2
        );
    }
    let both = format!("{},{}", routing_to(0), routing_to(1));
    assert_eq!(count(&node, &format!("/two/_count?routing={both}")), 4);
    assert_eq!(count(&node, "/two/_count?routing="), 4);
    let one = format!("/compat-term/_count?routing={}", routing_to(1));
    assert_eq!(count(&node, &one), count(&node, "/compat-term/_count"));
}

#[test]
fn a_count_accepts_a_preference_and_ignores_it() {
    let node = two_shards();
    assert_eq!(count(&node, "/two/_count?preference=_local"), 4);
}
