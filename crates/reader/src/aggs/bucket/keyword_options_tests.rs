//! The keyword-only options of `terms` parsed.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::keyword_options::{KeywordOptions, TermFilter};
use super::terms::TermsSpec;
use crate::dsl::parse_chunks;

fn options(terms_body: &str) -> KeywordOptions {
    let body = format!(r#"{{"aggs":{{"t":{{"terms":{terms_body}}}}}}}"#);
    let agg = parse_chunks([body.as_bytes()])
        .expect("parses")
        .aggs
        .remove(0);
    let spec: TermsSpec = *agg.terms.expect("a terms");
    spec.keyword
}

fn reason(terms_body: &str) -> String {
    let body = format!(r#"{{"aggs":{{"t":{{"terms":{terms_body}}}}}}}"#);
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    let mut text = failure.reason.clone();
    let mut cause = failure.cause.as_deref();
    while let Some(c) = cause {
        text.push_str(" | ");
        text.push_str(&c.reason);
        cause = c.cause.as_deref();
    }
    text
}

fn set(values: &[&str]) -> TermFilter {
    TermFilter::Set(values.iter().map(|v| (*v).to_owned()).collect())
}

fn regex(pattern: &str) -> TermFilter {
    TermFilter::Regex(pattern.to_owned())
}

// AC-1
#[test]
fn no_keyword_option_leaves_the_defaults() {
    assert!(options(r#"{"field":"tag"}"#).is_empty());
}

#[test]
fn include_and_exclude_are_a_regex_or_a_sorted_set() {
    let o = options(r#"{"field":"tag","include":"t1.*","exclude":"t12"}"#);
    assert_eq!(o.include, Some(regex("t1.*")));
    assert_eq!(o.exclude, Some(regex("t12")));
    let o = options(r#"{"field":"tag","include":["b","a","b",3],"exclude":["z"]}"#);
    assert_eq!(o.include, Some(set(&["3", "a", "b"])));
    assert_eq!(o.exclude, Some(set(&["z"])));
    assert!(o.filters());
    // Either order.
    let o = options(r#"{"field":"tag","exclude":["z"],"include":["a"]}"#);
    assert_eq!(
        (o.include, o.exclude),
        (Some(set(&["a"])), Some(set(&["z"])))
    );
    let o = options(r#"{"field":"tag","exclude":"z"}"#);
    assert_eq!((o.include, o.exclude), (None, Some(regex("z"))));
}

#[test]
fn missing_is_a_term() {
    assert_eq!(
        options(r#"{"field":"tag","missing":"none"}"#)
            .missing
            .as_deref(),
        Some("none")
    );
}

#[test]
fn a_regex_and_a_set_do_not_mix() {
    assert_eq!(
        reason(r#"{"field":"tag","include":"a.*","exclude":["b"]}"#),
        "[1:62] [terms] failed to parse field [exclude] | Cannot mix a regex-based include with a set-based method"
            .replace("[1:62]", &location(r#"{"field":"tag","include":"a.*","exclude":["b"]}"#))
    );
    assert!(
        reason(r#"{"field":"tag","exclude":["b"],"include":"a.*"}"#)
            .ends_with("Cannot mix a regex-based include with a set-based method")
    );
}

/// The location the parser reports for `terms_body`'s failure.
fn location(terms_body: &str) -> String {
    let body = format!(r#"{{"aggs":{{"t":{{"terms":{terms_body}}}}}}}"#);
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    let end = failure.reason.find(']').expect("a location");
    failure.reason[..=end].to_owned()
}

#[test]
fn bad_filters_fail_as_opensearch_fails_them() {
    assert!(
        reason(r#"{"field":"tag","include":5}"#)
            .ends_with("Unrecognized token for an include [VALUE_NUMBER]")
    );
    assert!(
        reason(r#"{"field":"tag","exclude":{"partition":0}}"#)
            .ends_with("Unrecognized token for an exclude [START_OBJECT]")
    );
    assert!(
        reason(r#"{"field":"tag","include":[["a"]]}"#)
            .ends_with("Array elements in include/exclude clauses should be string values")
    );
}

#[test]
fn partitions_and_non_string_missing_are_refused_as_not_supported_yet() {
    for body in [
        r#"{"field":"tag","include":{"partition":0,"num_partitions":2}}"#,
        r#"{"field":"tag","missing":5}"#,
        r#"{"field":"tag","include":[null]}"#,
    ] {
        assert!(
            reason(body).ends_with("is not supported by this node yet"),
            "{body}: {}",
            reason(body)
        );
    }
}
