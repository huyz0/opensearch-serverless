//! docs/specs/field-caps-api: `_field_caps` end to end.

use crate::support::{Node, counts, request, start};
#[cfg(feature = "coordinator")]
use crate::support::{put_shard_copy, put_variant};

fn caps(node: &Node, path: &str) -> (u16, String) {
    let r = request(node.addr(), "GET", path, None);
    (r.status, r.text())
}

const TITLE: &str = r#""title":{"text":{"type":"text","metadata_field":false,"searchable":true,"aggregatable":false}}"#;
const TAG: &str = r#""tag":{"keyword":{"type":"keyword","metadata_field":false,"searchable":true,"aggregatable":true}}"#;
const YEAR: &str = r#""year":{"integer":{"type":"integer","metadata_field":false,"searchable":true,"aggregatable":true}}"#;
const PUBLISHED: &str = r#""published":{"date":{"type":"date","metadata_field":false,"searchable":true,"aggregatable":true}}"#;

fn variant(node: &Node, name: &str, from: &str, to: &str) {
    crate::support::put_variant(&node.store, "compat-term", name, |j| j.replace(from, to));
}

// AC-1
#[test]
fn every_mapped_field_is_listed_with_its_capabilities() {
    let node = start();
    let (status, body) = caps(&node, "/compat-term/_field_caps?fields=*");
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        format!(r#"{{"indices":["compat-term"],"fields":{{{PUBLISHED},{TAG},{TITLE},{YEAR}}}}}"#)
    );
}

// AC-2
#[test]
fn fields_takes_names_and_patterns_and_leaves_out_what_matches_nothing() {
    let node = start();
    let (_, body) = caps(&node, "/compat-term/_field_caps?fields=ta*,year,nope");
    assert_eq!(
        body,
        format!(r#"{{"indices":["compat-term"],"fields":{{{TAG},{YEAR}}}}}"#)
    );
    let (_, body) = caps(&node, "/compat-term/_field_caps?fields=*ear,t*e,p*b*d,y*x");
    assert_eq!(
        body,
        format!(r#"{{"indices":["compat-term"],"fields":{{{PUBLISHED},{TITLE},{YEAR}}}}}"#)
    );
    let (status, body) = caps(&node, "/compat-term/_field_caps?fields=zzz");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, r#"{"indices":["compat-term"],"fields":{}}"#);
}

// AC-3
#[test]
fn index_doc_values_and_fielddata_decide_searchable_and_aggregatable() {
    let node = start();
    variant(
        &node,
        "caps",
        r#"\"year\":{\"type\":\"integer\"}"#,
        r#"\"year\":{\"type\":\"integer\",\"index\":false,\"doc_values\":false}"#,
    );
    let (_, body) = caps(&node, "/caps/_field_caps?fields=year");
    assert!(
        body.contains(r#""searchable":false,"aggregatable":false"#),
        "{body}"
    );
    variant(
        &node,
        "caps2",
        r#"\"title\":{\"type\":\"text\"}"#,
        r#"\"title\":{\"type\":\"text\",\"fielddata\":true}"#,
    );
    let (_, body) = caps(&node, "/caps2/_field_caps?fields=title");
    assert!(
        body.contains(r#""searchable":true,"aggregatable":true"#),
        "{body}"
    );
}

// AC-4
#[test]
fn several_indices_merge_by_type_and_name_the_indices_that_differ() {
    let node = start();
    variant(
        &node,
        "other",
        r#"\"year\":{\"type\":\"integer\"}"#,
        r#"\"year\":{\"type\":\"long\"},\"extra\":{\"type\":\"keyword\"}"#,
    );
    variant(
        &node,
        "dark",
        r#"\"tag\":{\"type\":\"keyword\"}"#,
        r#"\"tag\":{\"type\":\"keyword\",\"index\":false}"#,
    );
    let (status, body) = caps(
        &node,
        "/compat-term,other/_field_caps?fields=year,title,extra",
    );
    assert_eq!(status, 200, "{body}");
    assert!(
        body.starts_with(r#"{"indices":["compat-term","other"],"fields":{"#),
        "{body}"
    );
    assert!(
        body.contains(r#""integer":{"type":"integer","metadata_field":false,"searchable":true,"aggregatable":true,"indices":["compat-term"]}"#),
        "{body}"
    );
    assert!(
        body.contains(r#""long":{"type":"long","metadata_field":false,"searchable":true,"aggregatable":true,"indices":["other"]}"#),
        "{body}"
    );
    assert!(body.contains(TITLE), "{body}");
    assert!(body.contains(r#""extra":{"keyword":{"type":"keyword","metadata_field":false,"searchable":true,"aggregatable":true}}"#), "{body}");
    let (_, body) = caps(&node, "/compat-term,dark/_field_caps?fields=tag");
    assert!(
        body.contains(
            r#""searchable":false,"aggregatable":true,"non_searchable_indices":["dark"]"#
        ),
        "{body}"
    );
}

// AC-5
#[test]
fn a_call_without_fields_and_an_unknown_parameter_are_refused() {
    let node = start();
    let (status, body) = caps(&node, "/compat-term/_field_caps");
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("action_request_validation_exception"),
        "{body}"
    );
    assert!(body.contains("no fields specified"), "{body}");
    let (status, body) = caps(&node, "/compat-term/_field_caps?fields=*&q=a");
    assert_eq!(status, 400, "{body}");
}

// AC-6
#[test]
fn an_unknown_index_is_not_found_and_a_pattern_is_refused() {
    let node = start();
    let (status, body) = caps(&node, "/no-such-index/_field_caps?fields=*");
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("index_not_found_exception"), "{body}");
    for path in [
        "/comp*/_field_caps?fields=*",
        "/_all/_field_caps?fields=*",
        "/_field_caps?fields=*",
    ] {
        let (status, body) = caps(&node, path);
        assert_eq!(status, 400, "{path}: {body}");
        assert!(
            body.contains("illegal_argument_exception"),
            "{path}: {body}"
        );
        assert!(
            body.contains("not supported by this node yet"),
            "{path}: {body}"
        );
    }
}

// AC-7
#[cfg(feature = "coordinator")]
#[test]
fn an_index_of_two_shards_answers_as_one_of_one() {
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    let (status, two) = caps(&node, "/two/_field_caps?fields=*");
    assert_eq!(status, 200, "{two}");
    let (_, one) = caps(&node, "/compat-term/_field_caps?fields=*");
    assert_eq!(two, one.replace(r#"["compat-term"]"#, r#"["two"]"#));
}

// AC-8
#[test]
fn get_and_post_answer_alike_and_a_warm_call_makes_no_request() {
    let node = start();
    let (status, got) = caps(&node, "/compat-term/_field_caps?fields=*");
    assert_eq!(status, 200, "{got}");
    let warm = counts(&node.store);
    let post = request(
        node.addr(),
        "POST",
        "/compat-term/_field_caps?fields=*",
        None,
    );
    assert_eq!((post.status, post.text()), (200, got));
    assert_eq!(counts(&node.store), warm);
}
