//! docs/specs/search-several-indexes: `_search` over a list of indexes and an
//! alias of several.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use oss_node::search::alias::AliasSource;
use oss_rest::RestError;

use oss_blob::S3Op;
use oss_node::search_slots::{Config, Gated, SearchSlots};

use crate::support::{Node, counts, put_shard_copy, put_variant, request, start};

struct Table(Mutex<HashMap<String, Vec<String>>>);

impl AliasSource for Table {
    fn members(&self, name: &str, _buf: &mut [u8]) -> Result<Option<Vec<String>>, RestError> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }
}

fn node_with(aliases: &[(&str, &[&str])]) -> Node {
    let node = start();
    let table = Table(Mutex::new(
        aliases
            .iter()
            .map(|(n, m)| ((*n).to_owned(), m.iter().map(|s| (*s).to_owned()).collect()))
            .collect(),
    ));
    assert!(node.service.set_aliases(Arc::new(table)));
    node
}

fn post(node: &Node, path: &str, body: &str) -> (u16, String) {
    let r = request(node.addr(), "POST", path, Some(body.as_bytes()));
    (r.status, r.text())
}

const ALL: &str = r#"{"query":{"match_all":{}},"size":100}"#;

/// The number after `key` in `text`.
fn number(text: &str, key: &str) -> u64 {
    let from = text.find(key).unwrap_or_else(|| panic!("{key} in {text}")) + key.len();
    text[from..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|d| d.parse().ok())
        .expect("a number")
}

fn ids_of(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(r#""_id":""#) {
        rest = &rest[at + 7..];
        let end = rest.find('"').expect("end");
        found.push(rest[..end].to_owned());
    }
    found
}

fn indexes_of(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(r#""_index":""#) {
        rest = &rest[at + 10..];
        let end = rest.find('"').expect("end");
        found.push(&rest[..end]);
    }
    found
}

// AC-1
#[test]
fn a_list_of_indexes_is_searched_as_one() {
    let node = start();
    let (_, a) = post(&node, "/compat-term/_search", ALL);
    let (_, b) = post(&node, "/compat-bool/_search", ALL);
    let (status, both) = post(&node, "/compat-term,compat-bool/_search", ALL);
    assert_eq!(status, 200, "{both}");
    let total = |t: &str| number(t, r#""total":{"value":"#);
    assert_eq!(total(&both), total(&a) + total(&b), "{both}");
    assert_eq!(
        number(&both, r#""_shards":{"total":"#),
        number(&a, r#""_shards":{"total":"#) + number(&b, r#""_shards":{"total":"#)
    );
    let names = indexes_of(&both);
    assert!(
        names.contains(&"compat-term") && names.contains(&"compat-bool"),
        "{both}"
    );
}

// AC-2
#[test]
fn an_alias_of_several_is_the_list_and_a_repeat_is_searched_once() {
    let node = node_with(&[("both", &["compat-term", "compat-bool"])]);
    let (_, list) = post(&node, "/compat-term,compat-bool/_search", ALL);
    let (status, aliased) = post(&node, "/both/_search", ALL);
    assert_eq!(status, 200, "{aliased}");
    let total = |t: &str| number(t, r#""total":{"value":"#);
    assert_eq!(total(&aliased), total(&list));
    let (_, again) = post(&node, "/both,compat-term/_search", ALL);
    assert_eq!(
        total(&again),
        total(&list),
        "an index named twice is searched once"
    );
}

// AC-3
#[test]
fn a_name_that_is_no_index_refuses_the_search() {
    let node = start();
    let (status, body) = post(&node, "/compat-term,no-such-index/_search", ALL);
    assert_eq!(status, 404, "{body}");
    assert!(
        body.contains("index_not_found_exception") && body.contains("no-such-index"),
        "{body}"
    );
}

// AC-4
#[test]
fn a_size_merges_across_the_indexes() {
    let node = start();
    let (_, whole) = post(&node, "/compat-term,compat-bool/_search", ALL);
    let (status, page) = post(
        &node,
        "/compat-term,compat-bool/_search",
        r#"{"query":{"match_all":{}},"size":3}"#,
    );
    assert_eq!(status, 200, "{page}");
    assert_eq!(ids_of(&page), ids_of(&whole)[..3], "{page}");
    assert_eq!(
        number(&page, r#""total":{"value":"#),
        number(&whole, r#""total":{"value":"#)
    );
}

// AC-5
#[test]
fn collapse_and_a_view_are_refused_over_several_indexes() {
    let node = start();
    let (status, body) = post(
        &node,
        "/compat-term,compat-bool/_search",
        r#"{"collapse":{"field":"title"}}"#,
    );
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(
        &node,
        "/compat-term,compat-bool/_search",
        r#"{"pit":{"id":"x"}}"#,
    );
    assert_eq!(status, 400, "{body}");
}

// AC-6
#[test]
fn one_name_answers_as_before() {
    let node = node_with(&[("one", &["compat-term"])]);
    let (_, plain) = post(&node, "/compat-term/_search", ALL);
    for path in ["/compat-term,compat-term/_search", "/one/_search"] {
        let (status, body) = post(&node, path, ALL);
        assert_eq!(status, 200, "{path}: {body}");
        assert_eq!(
            number(&body, r#""total":{"value":"#),
            number(&plain, r#""total":{"value":"#),
            "{path}"
        );
    }
}

fn total(text: &str) -> u64 {
    number(text, r#""total":{"value":"#)
}

/// `two`: two shards holding copies of `compat-term`'s documents.
fn with_two_shards() -> Node {
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    node
}

// routing
#[test]
fn routing_narrows_each_index_of_the_list() {
    let node = with_two_shards();
    let routing = oss_cluster::routing::DocumentRouting::legacy_scaled(2, 1, 2).expect("layout");
    let value = (0..64)
        .map(|n| format!("r{n}"))
        .find(|r| routing.shard_for(r, Some(r)) == Ok(0))
        .expect("a value");
    let (_, one) = post(&node, "/compat-term/_search", ALL);
    let (_, whole) = post(&node, "/two,compat-term/_search", ALL);
    assert_eq!(total(&whole), 3 * total(&one), "two shards and one");
    let (status, routed) = post(
        &node,
        &format!("/two,compat-term/_search?routing={value}"),
        ALL,
    );
    assert_eq!(status, 200, "{routed}");
    assert_eq!(
        total(&routed),
        2 * total(&one),
        "one shard of two, and the one of the other"
    );
}

// budget
#[test]
fn a_list_costs_what_its_indexes_cost_alone_and_lists_nothing() {
    let cold = |path: &str| {
        let node = start();
        let before = counts(&node.store);
        assert_eq!(post(&node, path, ALL).0, 200);
        let after = counts(&node.store);
        let mut spent = [0u64; 7];
        for (i, n) in spent.iter_mut().enumerate() {
            *n = after[i] - before[i];
        }
        spent
    };
    let a = cold("/compat-term/_search");
    let b = cold("/compat-bool/_search");
    let both = cold("/compat-term,compat-bool/_search");
    for (i, op) in S3Op::ALL.iter().enumerate() {
        assert_eq!(both[i], a[i] + b[i], "{op:?}: {both:?} vs {a:?} and {b:?}");
    }
    let list = S3Op::ALL
        .iter()
        .position(|o| *o == S3Op::List)
        .expect("list");
    assert_eq!(both[list], 0);
}

// bounds
#[test]
fn a_search_may_name_at_most_the_limit_of_indexes() {
    let node = start();
    let names: Vec<String> = (0..65).map(|n| format!("idx{n}")).collect();
    let (status, body) = post(&node, &format!("/{}/_search", names.join(",")), ALL);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("at most 64 indexes"), "{body}");
}

// admission
#[test]
fn a_list_search_takes_a_permit_like_any_search() {
    let node = start();
    let gate = Arc::new(SearchSlots::new(&Config {
        slots: 4,
        fair_share_pct: 25,
        wait_millis: 100,
        waiter_cap: 2,
    }));
    assert!(node.service.set_search_slots(Gated::new(
        Arc::clone(&gate),
        Box::new(|name| (name.to_owned(), 0)),
    )));
    assert_eq!(post(&node, "/compat-term,compat-bool/_search", ALL).0, 200);
    assert_eq!(gate.report().admitted, 1);
    assert_eq!(gate.report().in_use, 0, "the permit goes back");
}

// _count
#[test]
fn a_count_over_a_list_or_an_alias_of_several_counts_them_all() {
    let node = node_with(&[("both", &["compat-term", "compat-bool"])]);
    let count = |path: &str| {
        let (status, body) = post(&node, path, r#"{"query":{"match_all":{}}}"#);
        assert_eq!(status, 200, "{path}: {body}");
        number(&body, r#""count":"#)
    };
    let a = count("/compat-term/_count");
    let b = count("/compat-bool/_count");
    assert_eq!(count("/compat-term,compat-bool/_count"), a + b);
    assert_eq!(count("/both/_count"), a + b);
    assert_eq!(
        count("/both,compat-term/_count"),
        a + b,
        "an index twice is counted once"
    );
}

#[test]
fn a_count_over_a_list_with_a_missing_index_names_it_alone() {
    let node = start();
    let (status, body) = post(&node, "/compat-term,no-such-index/_count", "{}");
    assert_eq!(status, 404, "{body}");
    assert!(
        body.contains("no-such-index") && !body.contains("compat-term"),
        "{body}"
    );
}

fn totals(text: &str) -> Vec<u64> {
    let key = r#""total":{"value":"#;
    text.match_indices(key)
        .map(|(at, _)| {
            text[at + key.len()..]
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|d| d.parse().ok())
                .expect("a number")
        })
        .collect()
}

// _msearch
#[test]
fn an_msearch_item_over_a_list_or_an_alias_of_several_searches_them_all() {
    let node = node_with(&[("both", &["compat-term", "compat-bool"])]);
    let (_, a) = post(&node, "/compat-term/_search", ALL);
    let (_, b) = post(&node, "/compat-bool/_search", ALL);
    let sum = total(&a) + total(&b);
    let item = |index: &str| format!("{{\"index\":\"{index}\"}}\n{ALL}\n");
    let body = format!(
        "{}{}{}",
        item("compat-term,compat-bool"),
        item("both"),
        item("compat-term")
    );
    let (status, answer) = post(&node, "/_msearch", &body);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(totals(&answer), vec![sum, sum, total(&a)], "{answer}");
    let (status, answer) = post(
        &node,
        "/compat-term,compat-bool/_msearch",
        &format!("{{}}\n{ALL}\n"),
    );
    assert_eq!(status, 200, "{answer}");
    assert_eq!(totals(&answer), vec![sum], "{answer}");
    let (status, answer) = post(&node, "/both/_msearch", &format!("{{}}\n{ALL}\n"));
    assert_eq!(status, 200, "{answer}");
    assert_eq!(totals(&answer), vec![sum], "{answer}");
}

// _field_caps
#[test]
fn field_caps_over_a_list_or_an_alias_of_several_names_the_indexes() {
    let node = node_with(&[("both", &["compat-term", "compat-bool"])]);
    let caps = |path: &str| {
        let r = request(node.addr(), "GET", path, None);
        assert_eq!(r.status, 200, "{path}: {}", r.text());
        r.text()
    };
    let list = caps("/compat-term,compat-bool/_field_caps?fields=*");
    assert_eq!(caps("/both/_field_caps?fields=*"), list);
    assert!(
        list.contains("compat-term") && list.contains("compat-bool")
            || !list.contains(r#""indices""#),
        "{list}"
    );
    assert_eq!(
        caps("/both,compat-term/_field_caps?fields=*"),
        list,
        "an index twice is read once"
    );
}
