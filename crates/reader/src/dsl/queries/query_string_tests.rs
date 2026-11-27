//! Tests of `query_string`'s parser; compiled only under `#[cfg(test)]`.

use super::full_text::MultiType;
use super::query_string::QueryString;
use super::*;
use crate::dsl::parse_chunks;
use crate::dsl::query::Operator;

fn parsed(body: &str) -> (QueryString, f32) {
    match parse_chunks([format!(r#"{{"query":{{"query_string":{body}}}}}"#).as_bytes()])
        .map(|s| s.query)
    {
        Ok(Some(Query::More(m))) => match *m {
            More::QueryString(q, boost) => (*q, boost),
            other => panic!("parsed {other:?}"),
        },
        other => panic!("parsed {other:?}"),
    }
}

fn reason(body: &str) -> String {
    match parse_chunks([format!(r#"{{"query":{{"query_string":{body}}}}}"#).as_bytes()]) {
        Err(e) => e.into_failure().reason,
        Ok(s) => panic!("parsed {s:?}"),
    }
}

#[test]
fn the_defaults_are_those_of_opensearch() {
    let (q, boost) = parsed(r#"{"query":"fox"}"#);
    assert_eq!(q.query, "fox");
    assert!(q.fields.is_empty());
    assert_eq!(q.default_field, None);
    assert_eq!(q.operator, Operator::Or);
    assert_eq!(q.kind, MultiType::BestFields);
    assert_eq!(q.tie_breaker, None);
    assert_eq!(q.phrase_slop, 0);
    assert_eq!(q.allow_leading_wildcard, None);
    assert_eq!(q.lenient, None);
    assert_eq!(q.minimum_should_match, None);
    assert_eq!(q.fuzziness, Fuzziness::AUTO);
    assert_eq!(
        (
            q.fuzzy_prefix_length,
            q.fuzzy_max_expansions,
            q.fuzzy_transpositions
        ),
        (0, 50, true)
    );
    assert!(!q.escape);
    assert_eq!(boost, 1.0);
}

#[test]
fn every_key_is_read() {
    let (q, boost) = parsed(
        r#"{"query":"fox","fields":["title^2","tag"],"default_operator":"and","type":"most_fields",
            "tie_breaker":0.25,"phrase_slop":3,"allow_leading_wildcard":false,"lenient":true,
            "minimum_should_match":"75%","fuzziness":1,"fuzzy_prefix_length":2,
            "fuzzy_max_expansions":9,"fuzzy_transpositions":false,"escape":true,"boost":2.5,
            "analyzer":"standard","analyze_wildcard":false,"enable_position_increments":true,
            "max_determinized_states":100,"auto_generate_synonyms_phrase_query":true}"#,
    );
    assert_eq!(
        q.fields,
        vec![("title".to_owned(), 2.0), ("tag".to_owned(), 1.0)]
    );
    assert_eq!(q.operator, Operator::And);
    assert_eq!(q.kind, MultiType::MostFields);
    assert_eq!(q.tie_breaker, Some(0.25));
    assert_eq!(q.phrase_slop, 3);
    assert_eq!(q.allow_leading_wildcard, Some(false));
    assert_eq!(q.lenient, Some(true));
    assert_eq!(q.minimum_should_match.as_deref(), Some("75%"));
    assert_eq!(q.fuzziness, Fuzziness::Edits(1));
    assert_eq!(
        (
            q.fuzzy_prefix_length,
            q.fuzzy_max_expansions,
            q.fuzzy_transpositions
        ),
        (2, 9, false)
    );
    assert!(q.escape);
    assert_eq!(boost, 2.5);
    let (q, _) = parsed(r#"{"query":"fox","default_field":"title","type":"phrase"}"#);
    assert_eq!(q.default_field.as_deref(), Some("title"));
    assert_eq!(q.kind, MultiType::Phrase);
}

#[test]
fn the_last_list_of_fields_wins() {
    let (q, _) = parsed(r#"{"query":"fox","fields":["a"],"fields":["b"]}"#);
    assert_eq!(q.fields, vec![("b".to_owned(), 1.0)]);
}

#[test]
fn parse_errors_are_opensearchs() {
    assert_eq!(
        reason(r#"{"fields":["title"]}"#),
        "[query_string] must be provided with a [query]"
    );
    assert_eq!(
        reason(r#"{"aaa":1,"query":"fox"}"#),
        "[query_string] query does not support [aaa]"
    );
    assert_eq!(
        reason(r#"{"fields":"title","query":"fox"}"#),
        "[query_string] query does not support [fields]"
    );
    assert_eq!(
        reason(r#"{"query":"fox","default_operator":"xor"}"#),
        "No enum constant org.opensearch.index.query.Operator.XOR"
    );
    assert_eq!(
        reason(r#"{"query":"fox","type":"nonsense"}"#),
        "failed to parse [multi_match] query type [nonsense]. unknown type."
    );
    assert_eq!(
        reason(r#"{"query":"fox","fields":[null]}"#),
        "[query_string] field name in [fields] cannot be null"
    );
    assert_eq!(
        reason(r#"{"query":{"a":1}}"#),
        "[query_string] unknown token [START_OBJECT] after [query]"
    );
}

#[test]
fn what_is_not_served_is_refused() {
    for body in [
        r#"{"query":"x","analyzer":"simple"}"#,
        r#"{"query":"x","quote_analyzer":"standard"}"#,
        r#"{"query":"x","quote_field_suffix":".exact"}"#,
        r#"{"query":"x","rewrite":"constant_score"}"#,
        r#"{"query":"x","fuzzy_rewrite":"constant_score"}"#,
        r#"{"query":"x","time_zone":"+01:00"}"#,
        r#"{"query":"x","analyze_wildcard":true}"#,
        r#"{"query":"x","enable_position_increments":false}"#,
        r#"{"query":"x","type":"cross_fields"}"#,
        r#"{"query":"x","type":"phrase_prefix"}"#,
        r#"{"query":"x","type":"bool_prefix"}"#,
        r#"{"query":"x","boost":-1}"#,
    ] {
        assert!(
            reason(body).contains("is not supported by this node yet"),
            "{body}"
        );
    }
}

#[test]
fn null_options_that_are_ignored_stay_ignored() {
    let (q, _) = parsed(
        r#"{"query":"x","quote_field_suffix":null,"rewrite":null,"fuzzy_rewrite":null,
            "minimum_should_match":null}"#,
    );
    assert_eq!(q.minimum_should_match, None);
}

#[test]
fn an_array_is_only_the_fields_value() {
    assert_eq!(
        reason(r#"{"query":"x","type":["a"]}"#),
        "[query_string] query does not support [type]"
    );
    let (_, boost) = parsed(r#"{"query":"x","boost":0}"#);
    assert_eq!(boost, 0.0);
}
