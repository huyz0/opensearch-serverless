//! docs/specs/search-on-alias: `_search`, `_count` and `_msearch` on an alias of one index.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oss_node::search::alias::AliasSource;
use oss_rest::RestError;

use crate::support::{Node, counts, request, start};

/// The aliases a test names, and how often the source was asked.
struct Table {
    members: Mutex<HashMap<String, Vec<String>>>,
    asked: AtomicUsize,
}

impl AliasSource for Table {
    fn members(&self, name: &str, _buf: &mut [u8]) -> Result<Option<Vec<String>>, RestError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Ok(self.members.lock().unwrap().get(name).cloned())
    }
}

fn node_with(aliases: &[(&str, &[&str])]) -> (Node, Arc<Table>) {
    let node = start();
    let table = Arc::new(Table {
        members: Mutex::new(
            aliases
                .iter()
                .map(|(n, m)| ((*n).to_owned(), m.iter().map(|s| (*s).to_owned()).collect()))
                .collect(),
        ),
        asked: AtomicUsize::new(0),
    });
    assert!(
        node.service
            .set_aliases(Arc::clone(&table) as Arc<dyn AliasSource>)
    );
    (node, table)
}

fn post(node: &Node, path: &str, body: &str) -> (u16, String) {
    let r = request(node.addr(), "POST", path, Some(body.as_bytes()));
    (r.status, r.text())
}

const TERM: &str = r#"{"query":{"term":{"title":"brown"}}}"#;

// AC-1
#[test]
fn a_search_of_an_alias_of_one_index_answers_as_the_index_does() {
    let (node, _) = node_with(&[("live", &["compat-term"])]);
    let (status, direct) = post(&node, "/compat-term/_search", TERM);
    assert_eq!(status, 200, "{direct}");
    let (status, aliased) = post(&node, "/live/_search", TERM);
    assert_eq!(status, 200, "{aliased}");
    let took = |s: &str| {
        let at = s.find(r#""took":"#).expect("took");
        let end = s[at + 7..].find(',').expect("comma") + at + 7;
        format!("{}{}", &s[..at], &s[end..])
    };
    assert_eq!(took(&aliased), took(&direct));
    assert!(aliased.contains(r#""_index":"compat-term""#), "{aliased}");
    assert!(!aliased.contains(r#""_index":"live""#), "{aliased}");
}

// AC-2
#[test]
fn a_count_and_an_msearch_item_take_the_alias_too() {
    let (node, _) = node_with(&[("live", &["compat-term"])]);
    let (_, direct) = post(&node, "/compat-term/_count", "{}");
    let (status, aliased) = post(&node, "/live/_count", "{}");
    assert_eq!((status, aliased.as_str()), (200, direct.as_str()));
    let body = format!("{{\"index\":\"live\"}}\n{TERM}\n");
    let (status, answer) = post(&node, "/_msearch", &body);
    assert_eq!(status, 200, "{answer}");
    assert!(answer.contains(r#""_index":"compat-term""#), "{answer}");
    let body = format!("{{}}\n{TERM}\n");
    let (status, answer) = post(&node, "/live/_msearch", &body);
    assert_eq!(status, 200, "{answer}");
    assert!(answer.contains(r#""_index":"compat-term""#), "{answer}");
}

// AC-3
#[test]
fn an_alias_that_moved_or_was_removed_is_read_again() {
    let (node, table) = node_with(&[("live", &["compat-term"])]);
    assert_eq!(post(&node, "/live/_count", "{}").0, 200);
    table.members.lock().unwrap().clear();
    let (status, body) = post(&node, "/live/_count", "{}");
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("index_not_found_exception"), "{body}");
    assert!(body.contains("live"), "{body}");
}

// AC-4
#[test]
fn an_alias_of_two_indexes_is_refused() {
    let (node, _) = node_with(&[("both", &["compat-term", "compat-other"])]);
    let (status, body) = post(&node, "/both/_count", TERM);
    assert_eq!(status, 501, "{body}");
    assert!(body.contains("unsupported_alias_option"), "{body}");
}

// AC-5
#[test]
fn a_concrete_index_never_asks_the_source_and_adds_no_request() {
    let (node, table) = node_with(&[("live", &["compat-term"])]);
    assert_eq!(post(&node, "/compat-term/_search", TERM).0, 200);
    let warm = counts(&node.store);
    assert_eq!(post(&node, "/compat-term/_search", TERM).0, 200);
    assert_eq!(counts(&node.store), warm);
    assert_eq!(table.asked.load(Ordering::SeqCst), 0);
    // An alias costs the failed open of its own name, one GET, and the source's read;
    // the index it stands for is open already.
    assert_eq!(post(&node, "/live/_search", TERM).0, 200);
    let mut want = warm;
    want[0] += 1;
    assert_eq!(counts(&node.store), want);
    assert_eq!(table.asked.load(Ordering::SeqCst), 1);
}

// AC-6
#[test]
fn a_name_that_is_neither_is_not_found_and_no_source_changes_nothing() {
    let (node, table) = node_with(&[]);
    let (status, body) = post(&node, "/nothing/_search", TERM);
    assert_eq!(status, 404, "{body}");
    assert_eq!(table.asked.load(Ordering::SeqCst), 1);
    let bare = start();
    let (status, body) = post(&bare, "/live/_search", TERM);
    assert_eq!(status, 404, "{body}");
}

// AC-7
#[test]
fn field_caps_of_an_alias_names_the_index_it_stands_for() {
    let (node, _) = node_with(&[("live", &["compat-term"]), ("both", &["compat-term", "x"])]);
    let get = |path: &str| {
        let r = request(node.addr(), "GET", path, None);
        (r.status, r.text())
    };
    let (status, direct) = get("/compat-term/_field_caps?fields=*");
    assert_eq!(status, 200, "{direct}");
    let (status, aliased) = get("/live/_field_caps?fields=*");
    assert_eq!((status, aliased.as_str()), (200, direct.as_str()));
    assert!(
        aliased.starts_with(r#"{"indices":["compat-term"]"#),
        "{aliased}"
    );
    let (status, body) = get("/both/_field_caps?fields=*");
    assert_eq!(status, 501, "{body}");
}
