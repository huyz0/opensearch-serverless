//! docs/specs/search-fields-api: a search body's `fields` array through HTTP.

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

#[test]
fn a_hit_carries_the_source_values_of_the_fields_asked_for() {
    let (status, text) = post(
        r#"{"query":{"term":{"tag":"manual"}},"fields":["title","tag","year","published","missing"]}"#,
    );
    assert_eq!(status, 200, "{text}");
    for want in [
        r#""title":["Quick start guide"]"#,
        r#""tag":["manual"]"#,
        r#""year":[2010]"#,
        r#""published":["2010-01-01T00:00:00.000Z"]"#,
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
    assert!(!text.contains("missing"), "{text}");
    // The object sits between `_source` and the end of the hit.
    let source = text.find(r#""_source":"#).expect("a source");
    let fields = text.find(r#","fields":{"#).expect("fields");
    assert!(source < fields, "{text}");
    // A pattern reads every field.
    let (status, all) = post(r#"{"query":{"term":{"tag":"manual"}},"fields":["*"]}"#);
    assert_eq!(status, 200, "{all}");
    assert!(all.contains(r#""year":[2010]"#), "{all}");
    // No value, no object; and no hit, no refusal of a bad format.
    let (status, none) = post(r#"{"query":{"term":{"tag":"manual"}},"fields":["missing"]}"#);
    assert_eq!(status, 200, "{none}");
    assert!(!none.contains(r#""fields""#), "{none}");
    let (status, empty) =
        post(r#"{"query":{"term":{"tag":"nothing"}},"fields":[{"field":"tag","format":"x"}]}"#);
    assert_eq!(status, 200, "{empty}");
}

#[test]
fn a_fields_request_the_node_cannot_answer_is_refused() {
    let (status, text) = post(r#"{"fields":[{"field":"tag","format":"x"}]}"#);
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("Field [tag] of type [keyword] doesn't support formats."),
        "{text}"
    );
    for body in [
        r#"{"fields":["tag"],"docvalue_fields":["year"]}"#,
        r#"{"fields":[{"field":"published","format":"yyyy"}]}"#,
    ] {
        let (status, text) = post(body);
        assert_eq!(status, 400, "{body}: {text}");
        assert!(text.contains("is not supported by this node yet"), "{text}");
    }
}
