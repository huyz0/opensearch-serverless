//! docs/specs/stored-search-templates: a stored template named by `id`, end to
//! end against templates held in memory (the stored set is in the server tests).

use std::collections::HashMap;
use std::sync::Arc;

use oss_node::search::{TemplateFault, TemplateSource};

use crate::support::{Node, request, start};

struct Held(HashMap<&'static str, (&'static str, &'static str)>);

impl TemplateSource for Held {
    fn reserve_bytes(&self) -> usize {
        0
    }
    fn resolve(&self, id: &str) -> Result<String, TemplateFault> {
        match self.0.get(id) {
            Some(("mustache", source)) => Ok((*source).to_owned()),
            Some((lang, _)) => Err(TemplateFault::WrongLang((*lang).to_owned())),
            None if id == "down" => Err(TemplateFault::Unreadable("the store is down".into())),
            None => Err(TemplateFault::Missing),
        }
    }
}

const MATCH: &str = r#"{"query":{"match":{"title":"{{q}}"}}}"#;

fn node() -> Node {
    let node = start();
    let held = Held(HashMap::from([
        ("by-title", ("mustache", MATCH)),
        ("all", ("mustache", r#"{"query":{"match_all":{}}}"#)),
        ("broken", ("mustache", "{{#a}}x")),
        ("score", ("painless", "1")),
    ]));
    assert!(node.service.set_stored_templates(Arc::new(held)));
    node
}

fn post(node: &Node, path: &str, body: &str) -> (u16, String) {
    let r = request(node.addr(), "POST", path, Some(body.as_bytes()));
    (r.status, r.text())
}

fn hits(body: &str) -> String {
    body[body.find("\"hits\":{").expect("hits")..].to_owned()
}

// AC-1
#[test]
fn a_stored_template_searches_what_the_inline_template_does() {
    let node = node();
    let (status, stored) = post(
        &node,
        "/compat-term/_search/template",
        r#"{"id":"by-title","params":{"q":"quick fox"}}"#,
    );
    assert_eq!(status, 200, "{stored}");
    let inline = format!(
        r#"{{"source":{},"params":{{"q":"quick fox"}}}}"#,
        quoted(MATCH)
    );
    let (_, inline) = post(&node, "/compat-term/_search/template", &inline);
    assert_eq!(hits(&stored), hits(&inline));
}

fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

// AC-2
#[test]
fn render_takes_the_id_from_the_path_and_the_path_wins() {
    let node = node();
    let want = (
        200,
        r#"{"template_output":{"query":{"match":{"title":"fox"}}}}"#.to_owned(),
    );
    assert_eq!(
        post(
            &node,
            "/_render/template/by-title",
            r#"{"params":{"q":"fox"}}"#
        ),
        want
    );
    let r = request(
        node.addr(),
        "GET",
        "/_render/template/by-title",
        Some(br#"{"params":{"q":"fox"}}"#),
    );
    assert_eq!((r.status, r.text()), want);
    // The body's own id or source does not change what the path names.
    for body in [
        r#"{"id":"all","params":{"q":"fox"}}"#,
        r#"{"source":"{}","params":{"q":"fox"}}"#,
    ] {
        assert_eq!(
            post(&node, "/_render/template/by-title", body),
            want,
            "{body}"
        );
    }
}

// AC-3
#[test]
fn an_msearch_item_names_a_stored_template_and_a_missing_one_fails_alone() {
    let node = node();
    let body = concat!(
        "{\"index\":\"compat-term\"}\n{\"id\":\"all\"}\n",
        "{\"index\":\"compat-term\"}\n{\"id\":\"nope\"}\n",
        "{\"index\":\"compat-term\"}\n{\"id\":\"all\"}\n"
    );
    let (status, answer) = post(&node, "/_msearch/template", body);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer.matches("\"status\":200").count(), 2, "{answer}");
    assert!(
        answer.contains("unable to find script [nope] in cluster state"),
        "{answer}"
    );
    assert!(answer.contains("\"status\":404"), "{answer}");
}

// AC-4
#[test]
fn a_missing_id_is_404_and_another_language_is_400() {
    let node = node();
    let (status, body) = post(&node, "/compat-term/_search/template", r#"{"id":"nope"}"#);
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("resource_not_found_exception"), "{body}");
    assert!(
        body.contains("unable to find script [nope] in cluster state"),
        "{body}"
    );
    let (status, body) = post(&node, "/_render/template/score", "{}");
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("illegal_argument_exception") && body.contains("painless"),
        "{body}"
    );
}

// AC-5
#[test]
fn a_stored_template_that_does_not_compile_is_a_script_exception() {
    let node = node();
    let (status, body) = post(&node, "/_render/template/broken", "{}");
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("script_exception") && body.contains("mustache_exception"),
        "{body}"
    );
    assert!(
        body.contains("{{#a}}x"),
        "the stored source is the script: {body}"
    );
}

// AC-6
#[test]
fn the_later_of_id_and_source_in_a_body_is_the_template() {
    let node = node();
    let (_, body) = post(&node, "/_render/template", r#"{"id":"all","source":"{}"}"#);
    assert_eq!(body, r#"{"template_output":{}}"#);
    let (_, body) = post(
        &node,
        "/_render/template",
        r#"{"source":"{}","id":"by-title","params":{"q":"x"}}"#,
    );
    assert!(body.contains(r#""match":{"title":"x"}"#), "{body}");
}

// AC-8
#[test]
fn an_unreadable_set_is_503() {
    let node = node();
    let (status, body) = post(&node, "/_render/template/down", "{}");
    assert_eq!(status, 503, "{body}");
    assert!(body.contains("the store is down"), "{body}");
}
