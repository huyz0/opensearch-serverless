//! docs/specs/search-body-timeout: body keys of `_search` that are read and change nothing.

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

/// The answer without its `took`, which differs between two runs of one search.
fn untimed(text: &str) -> String {
    let from = text.find(r#""took":"#).expect("took") + 7;
    let end = from + text[from..].find(',').expect("end of took");
    format!("{}{}", &text[..from], &text[end..])
}

#[test]
fn a_body_timeout_changes_nothing_and_a_bad_one_is_refused() {
    let (status, plain) = post(r#"{"query":{"match_all":{}}}"#);
    assert_eq!(status, 200, "{plain}");
    for timeout in [r#""30s""#, r#""1.5m""#, "-1", "0", r#""100ms""#] {
        let body = format!(r#"{{"timeout":{timeout},"query":{{"match_all":{{}}}}}}"#);
        let (status, text) = post(&body);
        assert_eq!(status, 200, "{timeout}: {text}");
        assert_eq!(untimed(&text), untimed(&plain), "{timeout}");
    }
    for timeout in [r#""30""#, r#""soon""#, "true", "30"] {
        let (status, text) = post(&format!(r#"{{"timeout":{timeout}}}"#));
        assert_eq!(status, 400, "{timeout}: {text}");
        assert!(text.contains("is not supported by this node yet"), "{text}");
    }
    let (status, text) = post(r#"{"timeout":null}"#);
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("Unknown key for a VALUE_NULL in [timeout]."),
        "{text}"
    );
}
