//! docs/specs/analyze-api: `_analyze` end to end.

use crate::support::{Node, counts, request, start};

fn analyze(node: &Node, path: &str, body: &str) -> (u16, String) {
    let r = request(node.addr(), "POST", path, Some(body.as_bytes()));
    (r.status, r.text())
}

fn tok(t: &str, s: i32, e: i32, ty: &str, p: i32) -> String {
    format!(r#"{{"token":"{t}","start_offset":{s},"end_offset":{e},"type":"{ty}","position":{p}}}"#)
}

fn tokens(list: &[String]) -> String {
    format!(r#"{{"tokens":[{}]}}"#, list.join(","))
}

// AC-1
#[test]
fn the_standard_analyzer_lower_cases_words_and_types_them() {
    let node = start();
    let (status, body) = analyze(
        &node,
        "/_analyze",
        r#"{"text":"The Quick-Brown fox's 42 jumps"}"#,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        tokens(&[
            tok("the", 0, 3, "<ALPHANUM>", 0),
            tok("quick", 4, 9, "<ALPHANUM>", 1),
            tok("brown", 10, 15, "<ALPHANUM>", 2),
            tok("fox's", 16, 21, "<ALPHANUM>", 3),
            tok("42", 22, 24, "<NUM>", 4),
            tok("jumps", 25, 30, "<ALPHANUM>", 5),
        ])
    );
    let (_, named) = analyze(
        &node,
        "/_analyze",
        r#"{"analyzer":"standard","text":"The Quick-Brown fox's 42 jumps"}"#,
    );
    assert_eq!(named, body);
}

// AC-2
#[test]
fn the_keyword_analyzer_makes_one_token_of_the_text() {
    let node = start();
    let (status, body) = analyze(
        &node,
        "/_analyze",
        r#"{"analyzer":"keyword","text":"The Quick fox"}"#,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, tokens(&[tok("The Quick fox", 0, 13, "word", 0)]));
}

// AC-3
#[test]
fn an_array_of_texts_continues_positions_and_offsets_with_the_gaps() {
    let node = start();
    let text = r#"{"text":["a b","c"]}"#;
    let (_, body) = analyze(&node, "/_analyze", text);
    assert_eq!(
        body,
        tokens(&[
            tok("a", 0, 1, "<ALPHANUM>", 0),
            tok("b", 2, 3, "<ALPHANUM>", 1),
            tok("c", 4, 5, "<ALPHANUM>", 2),
        ])
    );
    let (status, body) = analyze(&node, "/compat-term/_analyze", text);
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        tokens(&[
            tok("a", 0, 1, "<ALPHANUM>", 0),
            tok("b", 2, 3, "<ALPHANUM>", 1),
            tok("c", 4, 5, "<ALPHANUM>", 102),
        ])
    );
}

// AC-4
#[test]
fn a_missing_or_empty_text_is_refused() {
    let node = start();
    for body in ["", "{}", r#"{"text":[]}"#, r#"{"analyzer":"keyword"}"#] {
        let (status, answer) = analyze(&node, "/_analyze", body);
        assert_eq!(status, 400, "{body}: {answer}");
        assert!(
            answer.contains("action_request_validation_exception"),
            "{body}: {answer}"
        );
        assert!(
            answer.contains("Validation Failed: 1: text is missing;"),
            "{body}: {answer}"
        );
    }
}

// AC-5
#[test]
fn unknown_analyzers_and_unsupported_keys_are_refused() {
    let node = start();
    let (status, answer) = analyze(&node, "/_analyze", r#"{"analyzer":"nope","text":"a"}"#);
    assert_eq!(status, 400, "{answer}");
    assert!(
        answer.contains("failed to find global analyzer [nope]"),
        "{answer}"
    );
    let (status, answer) = analyze(
        &node,
        "/compat-term/_analyze",
        r#"{"analyzer":"nope","text":"a"}"#,
    );
    assert_eq!(status, 400, "{answer}");
    assert!(
        answer.contains("failed to find analyzer [nope]"),
        "{answer}"
    );
    for body in [
        r#"{"analyzer":"whitespace","text":"a"}"#,
        r#"{"field":"title","text":"a"}"#,
        r#"{"tokenizer":"standard","text":"a"}"#,
        r#"{"filter":["lowercase"],"text":"a"}"#,
        r#"{"explain":true,"text":"a"}"#,
    ] {
        let (status, answer) = analyze(&node, "/_analyze", body);
        assert_eq!(status, 400, "{body}: {answer}");
        assert!(
            answer.contains("illegal_argument_exception"),
            "{body}: {answer}"
        );
        assert!(
            answer.contains("not supported by this node yet"),
            "{body}: {answer}"
        );
    }
}

// AC-6
#[test]
fn an_unknown_index_is_not_found_and_an_unknown_parameter_is_refused() {
    let node = start();
    let (status, answer) = analyze(&node, "/no-such-index/_analyze", r#"{"text":"a"}"#);
    assert_eq!(status, 404, "{answer}");
    assert!(answer.contains("index_not_found_exception"), "{answer}");
    let (status, answer) = analyze(&node, "/_analyze?text=a", r#"{"text":"a"}"#);
    assert_eq!(status, 400, "{answer}");
}

// AC-7
#[test]
fn more_than_the_token_limit_is_refused() {
    let node = start();
    let ok = vec!["w"; 10_000].join(" ");
    let (status, _) = analyze(&node, "/_analyze", &format!(r#"{{"text":"{ok}"}}"#));
    assert_eq!(status, 200);
    let over = vec!["w"; 10_001].join(" ");
    let (status, answer) = analyze(&node, "/_analyze", &format!(r#"{{"text":"{over}"}}"#));
    assert_eq!(status, 500, "{}", &answer[..answer.len().min(300)]);
    assert!(answer.contains("illegal_state_exception"), "{answer}");
    assert!(
        answer.contains("exceeded the allowed maximum of [10000]"),
        "{answer}"
    );
}

// AC-8
#[test]
fn get_and_post_answer_alike_and_a_warm_call_makes_no_request() {
    let node = start();
    let body = r#"{"text":"Hello World"}"#;
    let (status, post) = analyze(&node, "/compat-term/_analyze", body);
    assert_eq!(status, 200, "{post}");
    let warm = counts(&node.store);
    let get = request(
        node.addr(),
        "GET",
        "/compat-term/_analyze",
        Some(body.as_bytes()),
    );
    assert_eq!((get.status, get.text()), (200, post));
    assert_eq!(counts(&node.store), warm);
    let before = counts(&node.store);
    assert_eq!(analyze(&node, "/_analyze", body).0, 200);
    assert_eq!(counts(&node.store), before);
}
