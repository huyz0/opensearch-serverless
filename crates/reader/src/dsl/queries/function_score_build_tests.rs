//! Tests of `function_score` built over a mapping; compiled only under
//! `#[cfg(test)]`. The scores are the replay's (`fs ...` steps of
//! `fixtures/compat/search/match.json`); these check what it cannot name:
//! the node's settings, and the failures a shard reports.

use lucene_analysis::Analyzer;
use lucene_search::Clause;
use lucene_search::extended_query::ExtendedQuery;

use crate::dsl::parse_chunks;
use crate::dsl::queries::More;
use crate::dsl::queries::function_score_build::function_score;
use crate::dsl::query::Query;
use crate::failure::Failure;
use crate::lucene_query::QueryContext;
use crate::mapping::Mapping;

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"},"tag":{"type":"keyword"},
    "year":{"type":"integer"},"big":{"type":"long"},"published":{"type":"date"}}}"#;

/// The node's key (its functions, as the engine will run them), and
/// whether it reads the wrapped query's scores.
fn built(body: &str) -> Result<(String, bool), Failure> {
    let mapping = Mapping::from_json(MAPPING).expect("mapping");
    let analyzer = Analyzer::standard(None);
    let cx = QueryContext {
        mapping: &mapping,
        analyzer: &analyzer,
        max_doc: 5,
        knn: None,
        pinned: None,
        now_millis: None,
    };
    let text = format!(r#"{{"query":{{"function_score":{body}}}}}"#);
    let source = parse_chunks([text.as_bytes()]).expect("parses");
    let Some(Query::More(m)) = source.query else {
        panic!("not a function_score");
    };
    let More::FunctionScore(q, _) = *m else {
        panic!("not a function_score");
    };
    match function_score(&cx, &q)? {
        Clause::Extended(e) => match *e {
            ExtendedQuery::FunctionScore(f) => {
                Ok((f.source().describe(), f.source().needs_scores()))
            }
            _ => panic!("not a function score"),
        },
        _ => Ok((String::from("plain"), false)),
    }
}

fn key(body: &str) -> String {
    built(body).expect("builds").0
}

fn fails(body: &str) -> Failure {
    built(body).expect_err("fails")
}

#[test]
fn no_functions_is_the_query_alone() {
    assert_eq!(key(r#"{"query":{"match_all":{}}}"#), "plain");
    assert_eq!(key(r#"{"functions":[]}"#), "plain");
}

#[test]
fn a_replacing_weight_does_not_read_the_query_score() {
    let (_, needs) = built(r#"{"weight":2,"boost_mode":"replace"}"#).expect("builds");
    assert!(!needs);
    let (k, needs) = built(r#"{"weight":2}"#).expect("builds");
    assert!(needs);
    assert!(k.contains("Weight"), "{k}");
}

#[test]
fn the_settings_reach_the_node() {
    let k = key(r#"{"weight":2,"score_mode":"sum","boost_mode":"avg","max_boost":1.5}"#);
    assert!(k.contains("Avg/Sum/1.5/"), "{k}");
    let k = key(r#"{"functions":[{"filter":{"match_all":{}},"weight":2}]}"#);
    assert!(k.contains("Some("), "the filter is kept: {k}");
}

#[test]
fn a_field_value_factor_keeps_its_settings() {
    let k =
        key(r#"{"field_value_factor":{"field":"year","factor":2,"modifier":"sqrt","missing":4}}"#);
    assert!(
        k.contains(r#"FieldValue { field: "year", name: "year", numeric: Long, factor: 2.0, modifier: Sqrt, missing: Some(4.0) }"#),
        "{k}"
    );
}

#[test]
fn a_field_value_factor_on_an_unmapped_field_needs_missing() {
    let f = fails(r#"{"field_value_factor":{"field":"nothing"}}"#);
    assert_eq!(f.kind, "query_shard_exception");
    assert_eq!(
        f.cause.expect("cause").reason,
        "Unable to find a field mapper for field [nothing]. No 'missing' value defined."
    );
    assert!(
        key(r#"{"field_value_factor":{"field":"nothing","missing":2}}"#)
            .contains("missing: Some(2.0)")
    );
}

#[test]
fn a_field_value_factor_reads_numbers_and_dates_only() {
    for field in ["title", "tag"] {
        let f = fails(&format!(
            r#"{{"field_value_factor":{{"field":"{field}"}}}}"#
        ));
        assert_eq!(f.kind, "illegal_argument_exception");
        assert!(
            f.reason.starts_with("Text fields are not optimised"),
            "{field}"
        );
        assert!(f.reason.contains(&format!("on [{field}] in order")));
    }
    for field in ["year", "big", "published"] {
        assert!(
            key(&format!(
                r#"{{"field_value_factor":{{"field":"{field}"}}}}"#
            ))
            .contains(field)
        );
    }
}

#[test]
fn a_numeric_decay_is_processed_as_opensearch_processes_it() {
    // origin 2001, scale 5, decay 0.5, offset 0.
    let g = key(r#"{"gauss":{"year":{"origin":2001,"scale":5}}}"#);
    let scale = 25.0 / (2.0 * 0.5_f64.ln());
    assert!(
        g.contains(&format!("origin: 2001.0, scale: {scale:?}, offset: 0.0")),
        "{g}"
    );
    let e = key(r#"{"exp":{"year":{"origin":2001,"scale":5,"offset":1,"decay":0.3}}}"#);
    let scale = 0.3_f64.ln() / 5.0;
    assert!(e.contains(&format!("scale: {scale:?}, offset: 1.0")), "{e}");
    let l = key(r#"{"linear":{"year":{"origin":2001,"scale":5}}}"#);
    assert!(l.contains("scale: 10.0, offset: 0.0"), "{l}");
    assert!(l.contains("mode: Min"), "{l}");
}

#[test]
fn a_date_decay_reads_times() {
    let k = key(r#"{"gauss":{"published":{"origin":"2001-03-04","scale":"10d","offset":"5d"}}}"#);
    assert!(k.contains("origin: 983664000000.0"), "{k}");
    assert!(k.contains("offset: 432000000.0"), "{k}");
    for (text, millis) in [
        ("1500ms", 1500.0),
        ("2s", 2000.0),
        ("3m", 180_000.0),
        ("1h", 3_600_000.0),
        ("2d", 172_800_000.0),
        ("5000nanos", 0.0),
        ("5000000micros", 5000.0),
        ("2D", 172_800_000.0),
    ] {
        let k = key(&format!(
            r#"{{"linear":{{"published":{{"origin":"2001-03-04","scale":"1d","offset":"{text}"}}}}}}"#
        ));
        assert!(k.contains(&format!("offset: {millis:?}")), "{text}: {k}");
    }
    let k = key(r#"{"linear":{"published":{"origin":978307200000,"scale":"1d","offset":"0"}}}"#);
    assert!(k.contains("origin: 978307200000.0"), "{k}");
    assert!(k.contains("offset: 0.0"), "{k}");
}

#[test]
fn a_decay_fails_as_opensearch_fails() {
    for (body, kind, message) in [
        (
            r#"{"gauss":{"year":{"scale":5}}}"#,
            "parse_exception",
            "both [scale] and [origin] must be set for numeric fields.",
        ),
        (
            r#"{"gauss":{"year":{"origin":5}}}"#,
            "parse_exception",
            "both [scale] and [origin] must be set for numeric fields.",
        ),
        (
            r#"{"gauss":{"published":{"origin":"2001-03-04"}}}"#,
            "parse_exception",
            "both [scale] and [origin] must be set for numeric fields.",
        ),
        (
            r#"{"gauss":{"year":{"origin":1,"scale":1,"aaa":1}}}"#,
            "parse_exception",
            "parameter [aaa] not supported!",
        ),
        (
            r#"{"gauss":{"year":{"origin":1,"scale":1,"decay":1.5}}}"#,
            "illegal_argument_exception",
            "function_score : decay must be in the range [0..1].",
        ),
        (
            r#"{"gauss":{"year":{"origin":1,"scale":1,"decay":0}}}"#,
            "illegal_argument_exception",
            "function_score : decay must be in the range [0..1].",
        ),
        (
            r#"{"gauss":{"year":{"origin":1,"scale":1,"decay":1}}}"#,
            "illegal_argument_exception",
            "function_score : decay must be in the range [0..1].",
        ),
        (
            r#"{"gauss":{"year":{"origin":1,"scale":0}}}"#,
            "illegal_argument_exception",
            "function_score : scale must be > 0.0.",
        ),
        (
            r#"{"gauss":{"year":{"origin":1,"scale":1,"offset":-1}}}"#,
            "illegal_argument_exception",
            "function_score : offset must be > 0.0",
        ),
        (
            r#"{"gauss":{"year":{"origin":"x","scale":1}}}"#,
            "number_format_exception",
            "For input string: \"x\"",
        ),
        (
            r#"{"gauss":{"year":{"origin":1,"scale":"x"}}}"#,
            "number_format_exception",
            "For input string: \"x\"",
        ),
        (
            r#"{"gauss":{"year":{"origin":1,"scale":1,"offset":"x"}}}"#,
            "number_format_exception",
            "For input string: \"x\"",
        ),
        (
            r#"{"gauss":{"published":{"origin":"2001-03-04","scale":"x"}}}"#,
            "illegal_argument_exception",
            "failed to parse setting [DecayFunctionParser.scale] with value [x] as a time value: unit is missing or unrecognized",
        ),
        (
            r#"{"gauss":{"published":{"origin":"2001-03-04","scale":5}}}"#,
            "illegal_argument_exception",
            "failed to parse setting [DecayFunctionParser.scale] with value [5] as a time value: unit is missing or unrecognized",
        ),
        (
            r#"{"gauss":{"published":{"origin":"2001-03-04","scale":"1d","offset":"1y"}}}"#,
            "illegal_argument_exception",
            "failed to parse setting [DecayFunctionParser.offset] with value [1y] as a time value: unit is missing or unrecognized",
        ),
    ] {
        let f = fails(body);
        let f = if f.kind == "query_shard_exception" {
            *f.cause.expect("wrapped")
        } else {
            f
        };
        assert_eq!((f.kind, f.reason.as_str()), (kind, message), "{body}");
    }
}

#[test]
fn a_parse_failure_is_wrapped_as_a_query_shard_exception() {
    let f = fails(r#"{"gauss":{"year":{"scale":5}}}"#);
    assert_eq!(f.kind, "query_shard_exception");
    assert_eq!(
        f.reason,
        "failed to create query: both [scale] and [origin] must be set for numeric fields."
    );
}

#[test]
fn a_decay_needs_a_mapped_numeric_field_and_a_date_an_origin() {
    let f = fails(r#"{"gauss":{"nothing":{"origin":1,"scale":1}}}"#);
    assert_eq!(
        (f.kind, f.reason.as_str()),
        ("parsing_exception", "unknown field [nothing]")
    );
    for field in ["title", "tag"] {
        let f = fails(&format!(
            r#"{{"gauss":{{"{field}":{{"origin":1,"scale":1}}}}}}"#
        ));
        assert!(
            f.reason.ends_with("is not supported by this node yet"),
            "{}",
            f.reason
        );
    }
    let f = fails(r#"{"gauss":{"published":{"scale":"1d"}}}"#);
    assert!(f.reason.contains("without an origin"), "{}", f.reason);
    let f = fails(r#"{"gauss":{"published":{"origin":"now","scale":"1d"}}}"#);
    assert!(f.reason.contains("date math"), "{}", f.reason);
    let f = fails(r#"{"gauss":{"published":{"origin":"nope","scale":"1d"}}}"#);
    assert_eq!(f.kind, "parse_exception");
    let f = fails(r#"{"gauss":{"published":{"origin":"2001-03-04","scale":"xd"}}}"#);
    assert!(f.reason.contains("the time value [xd]"), "{}", f.reason);
}
