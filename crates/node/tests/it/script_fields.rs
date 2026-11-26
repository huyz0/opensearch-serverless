//! docs/specs/script-fields: `script_fields` in the expression language, end
//! to end over the compat blobs.

use crate::support::{Node, counts, request, start};

fn search(node: &Node, extra: &str) -> (u16, String) {
    let body =
        format!(r#"{{"size":10,"query":{{"match_all":{{}}}},"sort":[{{"year":"asc"}}],{extra}}}"#);
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

const DOUBLED: &str = r#""script_fields":{"doubled":{"script":{"lang":"expression","source":"doc['year'].value * k","params":{"k":2}}}}"#;

// AC-1
#[test]
fn a_script_field_is_written_for_each_hit() {
    let node = start();
    let (status, body) = search(&node, DOUBLED);
    assert_eq!(status, 200, "{body}");
    // Years sort 1999, 2001, 2002, 2002, 2010.
    for want in ["3998.0", "4002.0", "4004.0", "4020.0"] {
        assert!(
            body.contains(&format!(r#""fields":{{"doubled":[{want}]}}"#)),
            "{want}: {body}"
        );
    }
}

// AC-2
#[test]
fn a_script_field_sits_beside_docvalue_fields() {
    let node = start();
    let (status, body) = search(&node, &format!(r#"{DOUBLED},"docvalue_fields":["year"]"#));
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains(r#""fields":{"year":[1999],"doubled":[3998.0]}"#),
        "{body}"
    );
}

// AC-3
#[test]
fn too_many_script_fields_are_refused() {
    let node = start();
    let one = r#"{"script":{"lang":"expression","source":"1"}}"#;
    let many = (0..33)
        .map(|n| format!(r#""f{n}":{one}"#))
        .collect::<Vec<_>>()
        .join(",");
    let (status, body) = search(&node, &format!(r#""script_fields":{{{many}}}"#));
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("Trying to retrieve too many script_fields. Must be less than or equal to: [32] but was [33]"),
        "{body}"
    );
}

// AC-4
#[test]
fn what_is_not_served_is_refused() {
    let node = start();
    for (fields, reason) in [
        (
            r#""script_fields":{"a":{"script":"1"}}"#,
            "runs only lang [expression]",
        ),
        (
            r#""script_fields":{"a":{"script":{"lang":"painless","source":"1"}}}"#,
            "runs only lang [expression]",
        ),
        (
            r#""script_fields":{"a":{"script":{"lang":"expression","source":"_score"}}}"#,
            "reads [_score]",
        ),
        (
            r#""script_fields":{"a":{"script":{"lang":"expression","source":"1"},"ignore_failure":true}}"#,
            "[ignore_failure]",
        ),
    ] {
        let (status, body) = search(&node, fields);
        assert_eq!(status, 400, "{fields}: {body}");
        assert!(body.contains(reason), "{fields}: {body}");
    }
}

// AC-5
#[test]
fn a_script_field_adds_no_store_request() {
    let node = start();
    let cost = |extra: &str| {
        search(&node, extra);
        let before = counts(&node.store);
        search(&node, extra);
        counts(&node.store)
            .iter()
            .zip(before.iter())
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>()
    };
    assert_eq!(cost(DOUBLED), cost(r#""docvalue_fields":["year"]"#));
}
