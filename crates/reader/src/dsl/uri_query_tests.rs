//! Tests of `uri_query.rs`.

use super::parse_chunks;
use super::uri_query::UriQuery;

fn body(query_string: &str) -> super::Query {
    // ALLOC: a test body, a few bytes.
    parse_chunks([format!(r#"{{"query":{{"query_string":{query_string}}}}}"#).as_bytes()])
        .expect("parses")
        .query
        .expect("a query")
}

#[test]
fn q_alone_is_a_query_string_query() {
    let got = UriQuery {
        q: "title:fox AND dog",
        ..UriQuery::default()
    }
    .to_query()
    .expect("builds");
    assert_eq!(got, body(r#"{"query":"title:fox AND dog"}"#));
}

#[test]
fn the_companions_become_the_query_strings_keys() {
    let got = UriQuery {
        q: "fox dog",
        df: Some("title"),
        default_operator: Some("and"),
        analyzer: Some("standard"),
        analyze_wildcard: Some("false"),
        lenient: Some("false"),
    }
    .to_query()
    .expect("builds");
    let want = body(
        r#"{"query":"fox dog","default_field":"title","default_operator":"and","analyzer":"standard","analyze_wildcard":false,"lenient":false}"#,
    );
    assert_eq!(got, want);
}

#[test]
fn quotes_backslashes_and_control_characters_stay_in_the_text() {
    let got = UriQuery {
        q: "\"a b\" \\ \u{1}",
        ..UriQuery::default()
    }
    .to_query()
    .expect("builds");
    assert_eq!(got, body(r#"{"query":"\"a b\" \\ \u0001"}"#));
}

#[test]
fn a_flag_that_is_not_true_or_false_is_refused() {
    for (wildcard, lenient) in [(Some("yes"), None), (None, Some("1"))] {
        let err = UriQuery {
            q: "a",
            analyze_wildcard: wildcard,
            lenient,
            ..UriQuery::default()
        }
        .to_query()
        .expect_err("refused");
        assert_eq!(err.status(), 400);
    }
}

#[test]
fn analyze_wildcard_true_is_refused_as_the_query_refuses_it() {
    let err = UriQuery {
        q: "a*",
        analyze_wildcard: Some("true"),
        ..UriQuery::default()
    }
    .to_query()
    .expect_err("refused");
    assert_eq!(err.status(), 400);
}

#[test]
fn an_operator_or_analyzer_the_query_refuses_is_refused() {
    for (operator, analyzer) in [(Some("xor"), None), (None, Some("whitespace"))] {
        let err = UriQuery {
            q: "a",
            default_operator: operator,
            analyzer,
            ..UriQuery::default()
        }
        .to_query()
        .expect_err("refused");
        assert_eq!(err.status(), 400);
    }
}
