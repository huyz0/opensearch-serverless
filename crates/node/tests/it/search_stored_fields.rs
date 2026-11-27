//! docs/specs/search-stored-fields: the `stored_fields` key of a search body through HTTP.

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
fn none_drops_the_id_and_the_source_and_names_drop_the_source() {
    let (status, plain) = post(r#"{"query":{"term":{"tag":"manual"}}}"#);
    assert_eq!(status, 200, "{plain}");
    assert!(
        plain.contains(r#""_id":"3""#) && plain.contains(r#""_source":"#),
        "{plain}"
    );
    for body in [
        r#"{"query":{"term":{"tag":"manual"}},"stored_fields":"_none_"}"#,
        r#"{"query":{"term":{"tag":"manual"}},"stored_fields":["_none_"],"_source":false}"#,
    ] {
        let (status, text) = post(body);
        assert_eq!(status, 200, "{body}: {text}");
        assert!(
            text.contains(r#""hits":[{"_index":"compat-paging-sort","_score":"#),
            "{text}"
        );
        assert!(
            !text.contains(r#""_id""#) && !text.contains(r#""_source""#),
            "{text}"
        );
        assert!(
            text.contains(r#""total":{"value":1,"relation":"eq"}"#),
            "{text}"
        );
    }
    for body in [
        r#"{"query":{"term":{"tag":"manual"}},"stored_fields":["title","missing"]}"#,
        r#"{"query":{"term":{"tag":"manual"}},"stored_fields":[]}"#,
    ] {
        let (status, text) = post(body);
        assert_eq!(status, 200, "{body}: {text}");
        assert!(
            text.contains(r#""_id":"3""#) && !text.contains(r#""_source""#),
            "{text}"
        );
        assert!(!text.contains(r#""fields""#), "{text}");
    }
    let (status, text) = post(
        r#"{"query":{"term":{"tag":"manual"}},"stored_fields":["_source"],"_source":["tag"]}"#,
    );
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""_source":{"tag":"manual"}"#), "{text}");
}

#[test]
fn what_the_node_cannot_answer_is_refused() {
    for body in [
        r#"{"stored_fields":"_none_","_source":true}"#,
        r#"{"stored_fields":["_none_","title"]}"#,
        r#"{"stored_fields":5}"#,
    ] {
        let (status, text) = post(body);
        assert_eq!(status, 400, "{body}: {text}");
        assert!(text.contains("is not supported by this node yet"), "{text}");
    }
}
