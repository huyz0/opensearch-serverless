//! `_name` on the compound, full-text, nested and function_score queries;
//! compiled only under `#[cfg(test)]`.

use crate::dsl::{Query, parse_chunks};

fn named(inner: &str) -> Option<String> {
    let body = format!(r#"{{"query":{inner}}}"#);
    match parse_chunks([body.as_bytes()]).map(|s| s.query) {
        Ok(Some(Query::Named(n))) => Some(n.name.clone()),
        other => panic!("{inner}: {other:?}"),
    }
}

#[test]
fn every_query_kind_of_this_slice_takes_a_name() {
    for inner in [
        r#"{"constant_score":{"filter":{"match_all":{}},"_name":"n"}}"#,
        r#"{"dis_max":{"queries":[{"match_all":{}}],"_name":"n"}}"#,
        r#"{"match_none":{"_name":"n"}}"#,
        r#"{"boosting":{"positive":{"match_all":{}},"negative":{"match_all":{}},"negative_boost":1,"_name":"n"}}"#,
        r#"{"match_phrase":{"title":{"query":"a","_name":"n"}}}"#,
        r#"{"match_phrase_prefix":{"title":{"query":"a","_name":"n"}}}"#,
        r#"{"match_bool_prefix":{"title":{"query":"a","_name":"n"}}}"#,
        r#"{"multi_match":{"query":"a","fields":["title"],"_name":"n"}}"#,
        r#"{"query_string":{"query":"a","_name":"n"}}"#,
        r#"{"simple_query_string":{"query":"a","_name":"n"}}"#,
        r#"{"nested":{"path":"c","query":{"match_all":{}},"_name":"n"}}"#,
        r#"{"function_score":{"query":{"match_all":{}},"weight":2,"_name":"n"}}"#,
    ] {
        assert_eq!(named(inner).as_deref(), Some("n"), "{inner}");
    }
}

#[test]
fn a_name_that_is_not_a_string_is_refused_and_null_is_none() {
    let body = r#"{"query":{"match_phrase":{"title":{"query":"a","_name":1}}}}"#;
    assert!(parse_chunks([body.as_bytes()]).is_err());
    let body = r#"{"query":{"match_phrase":{"title":{"query":"a","_name":null}}}}"#;
    assert!(matches!(
        parse_chunks([body.as_bytes()]).map(|s| s.query),
        Ok(Some(Query::More(_)))
    ));
}
