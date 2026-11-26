//! Tests of the `rescore` parser; compiled only under `#[cfg(test)]`.

use lucene_search::rescorer::QueryRescoreMode;

use super::parse_chunks;
use super::rescore::{DEFAULT_WINDOW, Rescorer};

fn parse(body: &str) -> Result<Vec<Rescorer>, crate::failure::Failure> {
    let text = format!(r#"{{"rescore":{body}}}"#);
    parse_chunks([text.as_bytes()])
        .map(|s| s.rescore)
        .map_err(super::DslError::into_failure)
}

fn reason(body: &str) -> String {
    parse(body).expect_err("fails").reason
}

const MATCH: &str = r#"{"match_all":{}}"#;

#[test]
fn the_defaults_are_stocks() {
    let one = parse(&format!(r#"{{"query":{{"rescore_query":{MATCH}}}}}"#)).expect("parses");
    assert_eq!(one.len(), 1);
    let r = &one[0];
    assert_eq!(r.window, DEFAULT_WINDOW);
    assert_eq!((r.query_weight, r.rescore_query_weight), (1.0, 1.0));
    assert_eq!(r.mode, QueryRescoreMode::Total);
}

#[test]
fn every_key_and_an_array_are_read() {
    let body = format!(
        r#"[{{"window_size":50,"query":{{"rescore_query":{MATCH},"query_weight":0.7,"rescore_query_weight":1.2,"score_mode":"Multiply"}}}},{{"query":{{"rescore_query":{MATCH},"score_mode":"avg"}}}}]"#
    );
    let all = parse(&body).expect("parses");
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].window, 50);
    assert_eq!(all[0].query_weight, 0.7);
    assert_eq!(all[0].rescore_query_weight, 1.2);
    assert_eq!(all[0].mode, QueryRescoreMode::Multiply);
    assert_eq!(all[1].mode, QueryRescoreMode::Avg);
    assert_eq!(all[1].window, DEFAULT_WINDOW);
}

#[test]
fn what_stock_refuses_is_refused_as_stock_does() {
    for (body, message) in [
        (r#"{"window_size":5}"#, "missing rescore type"),
        (r#"{"x":5,"query":{}}"#, "rescore doesn't support [x]"),
        (
            r#"{"script":{}}"#,
            "Unknown key for a START_OBJECT in [script].",
        ),
        (
            r#"{"query":{"query_weight":2}}"#,
            "rescore_query cannot be null",
        ),
        (
            &format!(r#"{{"query":{{"rescore_query":{MATCH},"score_mode":"sum"}}}}"#),
            "illegal score_mode [sum]",
        ),
        (
            &format!(r#"{{"query":{{"rescore_query":{MATCH},"zzz":1}}}}"#),
            "[query] unknown field [zzz]",
        ),
        (
            &format!(r#"{{"window_size":0,"query":{{"rescore_query":{MATCH}}}}}"#),
            "a [rescore] [window_size] below 1 is not supported by this node yet",
        ),
    ] {
        assert!(reason(body).ends_with(message), "{body}: {}", reason(body));
    }
}
