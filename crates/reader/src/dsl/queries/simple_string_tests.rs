//! Tests of `simple_query_string`'s parser; compiled only under
//! `#[cfg(test)]`.

use super::simple_string::{ALL_FLAGS, SimpleQueryString};
use super::*;
use crate::dsl::parse_chunks;
use crate::dsl::query::Operator;

fn parsed(body: &str) -> (SimpleQueryString, f32) {
    match parse_chunks([format!(r#"{{"query":{{"simple_query_string":{body}}}}}"#).as_bytes()])
        .map(|s| s.query)
    {
        Ok(Some(Query::More(m))) => match *m {
            More::SimpleQueryString(s, boost) => (*s, boost),
            other => panic!("parsed {other:?}"),
        },
        other => panic!("parsed {other:?}"),
    }
}

fn reason(body: &str) -> String {
    match parse_chunks([format!(r#"{{"query":{{"simple_query_string":{body}}}}}"#).as_bytes()]) {
        Err(e) => e.into_failure().reason,
        Ok(s) => panic!("parsed {s:?}"),
    }
}

#[test]
fn the_defaults_are_those_of_opensearch() {
    let (q, boost) = parsed(r#"{"query":"fox"}"#);
    assert_eq!(q.query, "fox");
    assert!(q.fields.is_empty());
    assert_eq!(q.flags, ALL_FLAGS);
    assert_eq!(q.operator, Operator::Or);
    assert_eq!(q.lenient, None);
    assert_eq!(q.minimum_should_match, None);
    assert_eq!(
        (
            q.fuzzy_prefix_length,
            q.fuzzy_max_expansions,
            q.fuzzy_transpositions
        ),
        (0, 50, true)
    );
    assert_eq!(boost, 1.0);
}

#[test]
fn every_key_is_read() {
    let (q, boost) = parsed(
        r#"{"query":"fox","fields":["title^2","tag"],"default_operator":"and","flags":"OR|AND",
            "lenient":true,"minimum_should_match":"75%","fuzzy_prefix_length":2,
            "fuzzy_max_expansions":9,"fuzzy_transpositions":false,"boost":2.5,
            "analyzer":"standard","all_fields":false,"locale":"en","analyze_wildcard":false,
            "auto_generate_synonyms_phrase_query":true,"lowercase_expanded_terms":false}"#,
    );
    assert_eq!(
        q.fields,
        vec![("title".to_owned(), 2.0), ("tag".to_owned(), 1.0)]
    );
    assert_eq!(q.operator, Operator::And);
    assert_eq!(q.flags, 4 | 1);
    assert_eq!(q.lenient, Some(true));
    assert_eq!(q.minimum_should_match.as_deref(), Some("75%"));
    assert_eq!(
        (
            q.fuzzy_prefix_length,
            q.fuzzy_max_expansions,
            q.fuzzy_transpositions
        ),
        (2, 9, false)
    );
    assert_eq!(boost, 2.5);
}

#[test]
fn flags_are_names_or_numbers() {
    let flags = |v: &str| parsed(&format!(r#"{{"query":"x","flags":{v}}}"#)).0.flags;
    assert_eq!(flags(r#""NONE""#), 0);
    assert_eq!(flags(r#""ALL""#), ALL_FLAGS);
    assert_eq!(
        flags(r#""AND|NOT|OR|PREFIX|PHRASE|PRECEDENCE|ESCAPE|WHITESPACE|FUZZY|NEAR""#),
        1 | 2 | 4 | 8 | 16 | 32 | 64 | 128 | 256 | 512
    );
    assert_eq!(flags(r#""PREFIX|FUZZY""#), 8 | 256);
    assert_eq!(flags("0"), 0);
    assert_eq!(flags("5"), 5);
    assert_eq!(flags("-3"), ALL_FLAGS);
    assert_eq!(
        flags("null").min(0),
        parsed(r#"{"query":"x","flags":null}"#).0.flags.min(0)
    );
}

#[test]
fn a_field_is_a_name_with_an_optional_boost() {
    let f = |v: &str| parsed(&format!(r#"{{"query":"x","fields":{v}}}"#)).0.fields;
    assert_eq!(
        f(r#"["a^0.5","b^2","c"]"#),
        vec![("a".into(), 0.5), ("b".into(), 2.0), ("c".into(), 1.0)]
    );
    assert_eq!(f("[]"), Vec::new());
    assert_eq!(f(r#"["a","b"]"#).len(), 2);
}

#[test]
fn parse_errors_are_opensearchs() {
    assert_eq!(
        reason(r#"{"fields":["a"]}"#),
        "[simple_query_string] query text missing"
    );
    assert_eq!(
        reason(r#"{"query":"x","zzz":1}"#),
        "[simple_query_string] unsupported field [zzz]"
    );
    assert_eq!(
        reason(r#"{"query":"x","fields":"a"}"#),
        "[simple_query_string] unsupported field [fields]"
    );
    assert_eq!(
        reason(r#"{"query":"x","analyzer":["a"]}"#),
        "[simple_query_string] query does not support [analyzer]"
    );
    assert_eq!(
        reason(r#"{"query":"x","flags":"BOGUS"}"#),
        "Unknown simple_query_string flag [BOGUS]"
    );
    assert_eq!(
        reason(r#"{"query":"x","default_operator":"xor"}"#),
        "No enum constant org.opensearch.index.query.Operator.XOR"
    );
    assert_eq!(
        reason(r#"{"query":"x","lenient":"maybe"}"#),
        "Failed to parse value [maybe] as only [true] or [false] are allowed."
    );
}

#[test]
fn what_is_not_served_is_refused() {
    for body in [
        r#"{"query":"x","analyze_wildcard":true}"#,
        r#"{"query":"x","analyzer":"whitespace"}"#,
        r#"{"query":"x","quote_field_suffix":".exact"}"#,
        r#"{"query":"x","_name":"n"}"#,
        r#"{"query":"x","boost":-1}"#,
        r#"{"query":"x","fields":["a^-1"]}"#,
        r#"{"query":"x","fields":[""]}"#,
    ] {
        assert!(
            reason(body).contains("not supported"),
            "{body}: {}",
            reason(body)
        );
    }
}

#[test]
fn a_zero_boost_is_allowed_and_an_object_is_no_value() {
    assert_eq!(parsed(r#"{"query":"x","boost":0}"#).1, 0.0);
    assert_eq!(
        parsed(r#"{"query":"x","fields":["a^0"]}"#).0.fields,
        vec![("a".into(), 0.0)]
    );
    assert_eq!(
        reason(r#"{"query":"x","lenient":{}}"#),
        "[simple_query_string] unknown token [START_OBJECT] after [lenient]"
    );
}
