//! docs/specs/search-template-api: `_render/template` and `_search/template` end to end.

use crate::support::{Node, counts, request, start};
#[cfg(feature = "coordinator")]
use crate::support::{put_shard_copy, put_variant};

fn post(node: &Node, path: &str, body: &str) -> (u16, String) {
    let r = request(node.addr(), "POST", path, Some(body.as_bytes()));
    (r.status, r.text())
}

fn render(node: &Node, body: &str) -> (u16, String) {
    post(node, "/_render/template", body)
}

// AC-1
#[test]
fn a_template_renders_over_its_params() {
    let node = start();
    let body = r#"{"source":"{\"query\":{\"term\":{\"title\":\"{{t}}\"}},\"size\":{{n}}}","params":{"t":"brown","n":3}}"#;
    assert_eq!(
        render(&node, body),
        (
            200,
            r#"{"template_output":{"query":{"term":{"title":"brown"}},"size":3}}"#.to_owned()
        )
    );
    let r = request(
        node.addr(),
        "GET",
        "/_render/template",
        Some(body.as_bytes()),
    );
    assert_eq!(r.status, 200, "{}", r.text());
}

// AC-3
#[test]
fn the_json_join_and_url_functions_render_as_java_does() {
    let node = start();
    let body = r#"{"source":"{\"a\":{{#toJson}}p{{/toJson}},\"b\":\"{{#join}}l{{/join}}\",\"c\":\"{{#url}}x y{{/url}}\"}","params":{"p":{"k":[1,2]},"l":["a","b"]}}"#;
    assert_eq!(
        render(&node, body),
        (
            200,
            r#"{"template_output":{"a":{"k":[1,2]},"b":"a,b","c":"x+y"}}"#.to_owned()
        )
    );
}

// AC-2
#[test]
fn a_search_template_hits_what_the_same_search_hits() {
    let node = start();
    let template =
        r#"{"source":"{\"query\":{\"match\":{\"title\":\"{{q}}\"}}}","params":{"q":"quick fox"}}"#;
    let (status, templated) = post(&node, "/compat-term/_search/template", template);
    assert_eq!(status, 200, "{templated}");
    let (_, searched) = post(
        &node,
        "/compat-term/_search",
        r#"{"query":{"match":{"title":"quick fox"}}}"#,
    );
    let hits = |s: &str| s[s.find("\"hits\":{").expect("hits")..].to_owned();
    assert_eq!(hits(&templated), hits(&searched));
}

// AC-2
#[cfg(feature = "coordinator")]
#[test]
fn a_two_shard_index_searches_through_the_coordinator() {
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    let template = r#"{"source":"{\"query\":{\"match_all\":{}}}"}"#;
    let (status, body) = post(&node, "/two/_search/template", template);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"total\":2"), "{body}");
}

// AC-4
#[test]
fn a_template_that_does_not_compile_is_a_script_exception() {
    let node = start();
    let (status, body) = render(&node, r#"{"source":"{{#a}}x","params":{}}"#);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("\"type\":\"script_exception\""), "{body}");
    assert!(body.contains("\"lang\":\"mustache\""), "{body}");
    assert!(body.contains("\"script_stack\":[]"), "{body}");
    assert!(body.contains("mustache_exception"), "{body}");
    let (status, body) = render(&node, r#"{"source":"{{>p}}","params":{}}"#);
    assert_eq!(status, 400, "{body}");
}

// AC-5
#[test]
fn a_request_that_is_no_template_is_refused_as_stock_refuses_it() {
    let node = start();
    let (status, body) = render(&node, "{}");
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("Validation Failed: 1: template is missing;"),
        "{body}"
    );
    for body in ["[1]", r#"{"nope":1}"#, r#"{"source":"x","params":3}"#, ""] {
        let (status, text) = render(&node, body);
        assert_eq!(status, 400, "{body}: {text}");
    }
}

// AC-6
#[test]
fn what_this_node_lacks_is_refused() {
    let node = start();
    for body in [
        r#"{"id":"stored"}"#,
        r#"{"source":"{}","profile":true}"#,
        r#"{"source":"{}","search_pipeline":"p"}"#,
    ] {
        let (status, text) = post(&node, "/compat-term/_search/template", body);
        assert_eq!(status, 400, "{body}: {text}");
        assert!(text.contains("not supported by this node yet"), "{text}");
    }
    let (status, text) = render(&node, r#"{"source":"{{#a}}{{/a}}","params":{}}"#);
    assert_eq!(status, 200, "{text}");
    let r = request(node.addr(), "POST", "/_render/template/stored", Some(b"{}"));
    assert_eq!(r.status, 400, "{}", r.text());
    let (status, text) = post(&node, "/_search/template", r#"{"source":"{}"}"#);
    assert_eq!(status, 400, "{text}");
}

// AC-7
#[test]
fn rendered_text_that_is_no_search_body_is_a_search_error() {
    let node = start();
    let (status, body) = post(
        &node,
        "/compat-term/_search/template",
        r#"{"source":"{\"nope\":1}"}"#,
    );
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(
        &node,
        "/no-such-index/_search/template",
        r#"{"source":"{}"}"#,
    );
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("index_not_found_exception"), "{body}");
}

// AC-8
#[test]
fn a_warm_index_adds_no_store_request() {
    let node = start();
    let body = r#"{"source":"{\"query\":{\"match_all\":{}}}"}"#;
    assert_eq!(post(&node, "/compat-term/_search/template", body).0, 200);
    let warm = counts(&node.store);
    assert_eq!(post(&node, "/compat-term/_search/template", body).0, 200);
    assert_eq!(counts(&node.store), warm);
}
