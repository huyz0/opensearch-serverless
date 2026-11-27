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
            ascending: true,
            nested: None,
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
        r#"{"highlight":{"fields":{"t":{"fragmenter":"span"}}}}"#,
        r#"{"aggs":{"t":{"adjacency_matrix":{"filters":{}}}}}"#,
    ] {
        assert!(
            reason(body).contains("is not supported by this node yet"),
            "{body}"
        );
    }
}

#[test]
fn a_highlight_is_read_with_its_fields() {
    let s = source(r#"{"highlight":{"fields":{"t":{},"u":{}}}}"#).expect("parses");
    let names: Vec<_> = s
        .highlight
        .expect("highlight")
        .fields
        .into_iter()
        .map(|f| f.0)
        .collect();
    assert_eq!(names, ["t", "u"]);
    assert!(source("{}").expect("parses").highlight.is_none());
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

#[test]
fn a_pit_gives_its_id_and_keep_alive_in_any_chunking() {
    let body = br#"{"pit":{"id":"AAAAAAAAAAAAAAAAAAAAAA","keep_alive":"1m"},"size":3}"#;
    let whole = parse_chunks([&body[..]]).expect("parses");
    assert_eq!(
        whole.pit,
        Some(PitRef {
            id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
            keep_alive: Some("1m".into()),
        })
    );
    assert_eq!(whole.size, 3);
    for cut in 1..body.len() {
        let (a, b) = body.split_at(cut);
        assert_eq!(parse_chunks([a, b]).expect("parses"), whole, "cut at {cut}");
    }
    let bare = source(r#"{"pit":{"id":"X","keep_alive":null}}"#).expect("parses");
    assert_eq!(
        bare.pit,
        Some(PitRef {
            id: "X".into(),
            keep_alive: None
        })
    );
    assert_eq!(source("{}").expect("parses").pit, None);
}

#[test]
fn a_pit_that_is_not_an_id_and_a_keep_alive_is_refused() {
    for body in [
        r#"{"pit":{}}"#,
        r#"{"pit":{"keep_alive":"1m"}}"#,
        r#"{"pit":{"id":7}}"#,
        r#"{"pit":{"id":"X","extra":1}}"#,
        r#"{"pit":{"id":"X","keep_alive":5}}"#,
        r#"{"pit":"X"}"#,
        r#"{"pit":["X"]}"#,
    ] {
        match source(body) {
            Err(e) => assert_eq!(e.status(), 400, "{body}"),
            Ok(s) => panic!("{body} parsed as {s:?}"),
        }
    }
    assert!(reason(r#"{"pit":{"id":"X","extra":1}}"#).contains("[pit] unknown field [extra]"));
    assert!(reason(r#"{"pit":{}}"#).contains("[pit] Required [id]"));
}

#[test]
fn a_body_names_a_search_pipeline_by_string_only() {
    let named = source(r#"{"search_pipeline":"cut","size":2}"#).expect("parses");
    assert_eq!(named.search_pipeline.as_deref(), Some("cut"));
    assert_eq!(source("{}").expect("parses").search_pipeline, None);
    for body in [
        r#"{"search_pipeline":""}"#,
        r#"{"search_pipeline":null}"#,
        r#"{"search_pipeline":7}"#,
        r#"{"search_pipeline":{"response_processors":[]}}"#,
    ] {
        assert!(source(body).is_err(), "{body}");
    }
}

#[test]
fn a_caller_that_runs_no_pipeline_refuses_one() {
    let named = source(r#"{"search_pipeline":"cut"}"#).expect("parses");
    assert!(named.refuse_pipeline().is_err());
    assert!(source("{}").expect("parses").refuse_pipeline().is_ok());
}

#[test]
fn a_body_timeout_is_read_as_a_time_value_and_changes_nothing() {
    for ok in [
        r#"{"timeout":"30s"}"#,
        r#"{"timeout":"1.5s"}"#,
        r#"{"timeout":"2M"}"#,
        r#"{"timeout":" 5 m "}"#,
        r#"{"timeout":"100ms"}"#,
        r#"{"timeout":"7d"}"#,
        r#"{"timeout":"9h"}"#,
        r#"{"timeout":"1000micros"}"#,
        r#"{"timeout":"5nanos"}"#,
        r#"{"timeout":-1}"#,
        r#"{"timeout":"-1"}"#,
        r#"{"timeout":0}"#,
        r#"{"timeout":"000"}"#,
    ] {
        assert_eq!(source(ok).expect(ok), SearchSource::default(), "{ok}");
    }
    for bad in [
        r#"{"timeout":"30"}"#,
        r#"{"timeout":"s"}"#,
        r#"{"timeout":"1x"}"#,
        r#"{"timeout":"-2s"}"#,
        r#"{"timeout":""}"#,
        r#"{"timeout":true}"#,
        r#"{"timeout":30}"#,
    ] {
        assert!(
            reason(bad).ends_with("is not supported by this node yet"),
            "{bad}: {}",
            reason(bad)
        );
    }
    assert_eq!(
        reason(r#"{"timeout":null}"#),
        "Unknown key for a VALUE_NULL in [timeout]."
    );
    assert_eq!(
        reason(r#"{"timeout":{}}"#),
        "Unknown key for a START_OBJECT in [timeout]."
    );
}

#[test]
fn profile_false_and_stats_are_read_and_change_nothing() {
    for body in [
        r#"{"profile":false}"#,
        r#"{"stats":["a","b"]}"#,
        r#"{"stats":[]}"#,
        r#"{"profile":false,"stats":["a"],"size":3}"#,
    ] {
        let parsed = source(body).unwrap_or_else(|_| panic!("{body} is read"));
        let mut plain = SearchSource::default();
        if body.contains("size") {
            plain.size = 3;
        }
        assert_eq!(parsed, plain, "{body}");
    }
}

#[test]
fn profile_true_and_a_stats_element_that_is_not_a_string_are_refused() {
    for body in [
        r#"{"profile":true}"#,
        r#"{"stats":[1]}"#,
        r#"{"stats":[{"a":1}]}"#,
    ] {
        assert!(
            reason(body).contains("is not supported by this node yet"),
            "{body}"
        );
    }
}

#[test]
fn stored_fields_none_drops_the_id_and_the_source() {
    for body in [
        r#"{"stored_fields":"_none_"}"#,
        r#"{"stored_fields":["_none_"]}"#,
    ] {
        let s = source(body).expect("parses");
        assert!(s.source.skip && s.source.no_id, "{body}");
    }
    let s = source(r#"{"stored_fields":"_none_","_source":false}"#).expect("parses");
    assert!(s.source.skip && s.source.no_id);
    for body in [
        r#"{"stored_fields":"_none_","_source":true}"#,
        r#"{"stored_fields":"_none_","_source":["a"]}"#,
        r#"{"stored_fields":"_none_","fields":["a"]}"#,
        r#"{"stored_fields":["_none_","a"]}"#,
        r#"{"stored_fields":3}"#,
        r#"{"stored_fields":[1]}"#,
    ] {
        assert!(
            reason(body).contains("is not supported by this node yet"),
            "{body}"
        );
    }
}

#[test]
fn stored_fields_names_keep_the_id_and_drop_the_source_unless_asked_for() {
    let skipped = |body: &str| {
        let s = source(body).expect("parses");
        assert!(!s.source.no_id, "{body}");
        s.source.skip
    };
    assert!(skipped(r#"{"stored_fields":"a"}"#));
    assert!(skipped(r#"{"stored_fields":["a","b"]}"#));
    assert!(skipped(r#"{"stored_fields":[]}"#));
    assert!(skipped(r#"{"stored_fields":["a"],"_source":false}"#));
    // `_source` as a name, or as a key, keeps the source.
    assert!(!skipped(r#"{"stored_fields":["a","_source"]}"#));
    assert!(!skipped(r#"{"stored_fields":["_source"],"_source":false}"#));
    assert!(!skipped(r#"{"stored_fields":["a"],"_source":true}"#));
    assert!(!skipped(r#"{"_source":["a"],"stored_fields":["b"]}"#));
    let s = source(r#"{"stored_fields":["a"],"_source":["a"]}"#).expect("parses");
    assert_eq!(s.source.includes, ["a"]);
    // Without the key nothing changes.
    assert_eq!(
        source("{}").expect("parses").source,
        SourceFilter::default()
    );
}

#[test]
fn an_empty_ext_is_read_and_a_section_is_refused() {
    assert_eq!(source(r#"{"ext":{}}"#).ok(), Some(SearchSource::default()));
    assert!(reason(r#"{"ext":{"rerank":{"a":1}}}"#).contains("is not supported by this node yet"));
}
