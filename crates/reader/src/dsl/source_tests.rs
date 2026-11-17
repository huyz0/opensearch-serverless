//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::*;
use crate::dsl::parse_chunks;

fn source(body: &str) -> Result<SearchSource, DslError> {
    parse_chunks([body.as_bytes()])
}

fn reason(body: &str) -> String {
    match source(body) {
        Err(e) => e.into_failure().reason,
        Ok(s) => panic!("parsed {s:?}"),
    }
}

#[test]
fn an_empty_body_is_the_default() {
    assert_eq!(source("").ok(), Some(SearchSource::default()));
    assert_eq!(source("  ").ok(), Some(SearchSource::default()));
    assert_eq!(source("{}").ok(), Some(SearchSource::default()));
}

#[test]
fn size_from_and_sort_in_the_body() {
    let s =
        source(r#"{"size":2,"from":"3","sort":"year","query":{"match_all":{}}}"#).expect("parses");
    assert_eq!((s.size, s.from), (2, 3));
    assert_eq!(
        s.sort,
        [SortKey::Field {
            field: "year".into(),
            ascending: true
        }]
    );
    assert!(s.query.is_some());
    let s = source(r#"{"sort":{"year":"desc"}}"#).expect("parses");
    assert_eq!(s.sort.len(), 1);
    let s = source(r#"{"sort":["_score"]}"#).expect("parses");
    assert_eq!(s.sort, [SortKey::Score { ascending: false }]);
}

#[test]
fn the_body_is_split_anywhere() {
    let body = br#"{"query":{"match":{"title":"a fairly long piece of text"}},"size":5}"#;
    let whole = parse_chunks([&body[..]]).expect("parses");
    for cut in 1..body.len() {
        let (a, b) = body.split_at(cut);
        assert_eq!(parse_chunks([a, b]).expect("parses"), whole, "cut at {cut}");
    }
}

#[test]
fn keys_opensearch_does_not_know_fail_with_their_token() {
    assert_eq!(
        reason(r#"{"query":{"match_all":{}},"nope":{}}"#),
        "Unknown key for a START_OBJECT in [nope]."
    );
    assert_eq!(
        reason(r#"{"nope":1}"#),
        "Unknown key for a VALUE_NUMBER in [nope]."
    );
    assert_eq!(
        reason(r#"{"nope":[]}"#),
        "Unknown key for a START_ARRAY in [nope]."
    );
    assert_eq!(
        reason(r#"{"query":"x"}"#),
        "Unknown key for a VALUE_STRING in [query]."
    );
    assert_eq!(
        reason(r#"[]"#),
        "Expected [START_OBJECT] but found [START_ARRAY]"
    );
    assert_eq!(
        reason(r#"1"#),
        "Expected [START_OBJECT] but found [VALUE_NUMBER]"
    );
}

#[test]
fn keys_opensearch_knows_are_refused_as_not_served() {
    for body in [
        r#"{"highlight":{"fields":{"t":{}}}}"#,
        r#"{"aggs":{"t":{"terms":{"field":"tag"}}}}"#,
        r#"{"search_after":[1]}"#,
    ] {
        assert!(
            reason(body).contains("is not supported by this node yet"),
            "{body}"
        );
    }
}

#[test]
fn negative_and_malformed_numbers() {
    assert_eq!(
        reason(r#"{"size":-1}"#),
        "[size] parameter cannot be negative, found [-1]"
    );
    assert_eq!(
        reason(r#"{"from":-2}"#),
        "[from] parameter cannot be negative, found [-2]"
    );
    assert_eq!(reason(r#"{"size":"x"}"#), "For input string: \"x\"");
    assert_eq!(reason(r#"{"size":true}"#), "For input string: \"true\"");
    assert!(reason("{").contains("Unexpected end-of-input"));
}

#[test]
fn url_parameters_override_the_body() {
    let mut s = source(r#"{"size":2,"from":3}"#).expect("parses");
    s.apply_params(Some("1"), Some("4")).expect("applies");
    assert_eq!((s.from, s.size), (1, 4));
    s.apply_params(None, None).expect("nothing");
    assert_eq!((s.from, s.size), (1, 4));
    let e = s.apply_params(None, Some("x")).expect_err("not a number");
    assert_eq!(
        e.into_failure().reason,
        "Failed to parse int parameter [size] with value [x]"
    );
    let e = s.apply_params(Some("-1"), None).expect_err("negative");
    assert_eq!(
        e.into_failure().reason,
        "[from] parameter cannot be negative, found [-1]"
    );
}

#[test]
fn an_unknown_key_is_located_where_the_recording_has_it() {
    // The bytes the errors fixture sent; OpenSearch answered at column 34,
    // the value's start.
    let f = source(r#"{"query":{"match_all":{}},"nope":{}}"#)
        .expect_err("refused")
        .into_failure();
    assert_eq!(
        f.meta,
        [
            ("line", crate::failure::Meta::Int(1)),
            ("col", crate::failure::Meta::Int(34))
        ]
    );
}

#[test]
fn a_token_after_the_main_object_is_refused_by_the_tokenizer() {
    // OpenSearch reads a second root value and refuses it as a
    // parsing_exception; this tokenizer stops at the first, a documented
    // difference.
    let e = source(r#"{"size":1} {}"#).expect_err("refused");
    let failure = e.into_failure();
    assert_eq!(failure.kind, "json_parse_exception");
    assert!(
        failure.reason.contains("after the root value"),
        "{}",
        failure.reason
    );
}

#[test]
fn track_total_hits_counts_up_to_a_number_or_not_at_all() {
    let tracked = |body: &str| source(body).expect("parses").track_total_hits;
    assert_eq!(tracked("{}"), Some(DEFAULT_TRACK_TOTAL_HITS));
    let exact = Some(u64::from(i32::MAX.cast_unsigned()));
    assert_eq!(tracked(r#"{"track_total_hits":true}"#), exact);
    assert_eq!(tracked(r#"{"track_total_hits":"true"}"#), exact);
    assert_eq!(tracked(r#"{"track_total_hits":false}"#), None);
    assert_eq!(tracked(r#"{"track_total_hits":"false"}"#), None);
    assert_eq!(tracked(r#"{"track_total_hits":-1}"#), None);
    assert_eq!(tracked(r#"{"track_total_hits":0}"#), Some(0));
    assert_eq!(tracked(r#"{"track_total_hits":3}"#), Some(3));
    assert_eq!(tracked(r#"{"track_total_hits":"3.7"}"#), Some(3));
    assert_eq!(tracked(r#"{"track_total_hits":2147483647}"#), exact);
    for below in ["-2", "-100"] {
        assert!(
            reason(&format!(r#"{{"track_total_hits":{below}}}"#))
                .ends_with("is not supported by this node yet"),
            "{below}"
        );
    }
    assert_eq!(
        reason(r#"{"track_total_hits":"yes"}"#),
        "For input string: \"yes\""
    );
    assert_eq!(
        reason(r#"{"track_total_hits":null}"#),
        "Unknown key for a VALUE_NULL in [track_total_hits]."
    );
}

#[test]
fn the_source_filter_is_read_from_a_value_an_object_or_an_array() {
    let filter = |body: &str| source(body).expect("parses").source;
    assert!(filter(r#"{"_source":false}"#).skip);
    assert_eq!(filter(r#"{"_source":"n*"}"#).includes, ["n*"]);
    assert_eq!(filter(r#"{"_source":["a","b"]}"#).includes, ["a", "b"]);
    let f = filter(r#"{"_source":{"includes":["a"],"excludes":"b"}}"#);
    assert_eq!(
        (f.includes, f.excludes),
        (vec!["a".to_owned()], vec!["b".to_owned()])
    );
    assert_eq!(
        reason(r#"{"_source":null}"#),
        "Unknown key for a VALUE_NULL in [_source]."
    );
}
