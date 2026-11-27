//! docs/specs/search-several-indexes: `_search` over a list of indexes and an
//! alias of several.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use oss_node::search::alias::AliasSource;
use oss_rest::RestError;

use crate::support::{Node, request, start};

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
    assert_eq!(indexes_of(&page).len(), 3, "{page}");
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
