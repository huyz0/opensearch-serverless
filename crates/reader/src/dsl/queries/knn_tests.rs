//! Tests of the `knn` parser and where it is found; compiled only under
//! `#[cfg(test)]`.

use super::*;
use crate::dsl::parse_chunks;
use crate::failure::Failure;

fn knn(body: &str) -> knn::Knn {
    match parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]).map(|s| s.query) {
        Ok(Some(Query::More(m))) => match *m {
            More::Knn(k) => *k,
            other => panic!("parsed {other:?}"),
        },
        other => panic!("parsed {other:?}"),
    }
}

fn failure(body: &str) -> Failure {
    match parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]) {
        Err(e) => e.into_failure(),
        Ok(s) => panic!("parsed {s:?}"),
    }
}

fn reason(params: &str) -> String {
    failure(&format!(r#"{{"knn":{{"v":{{{params}}}}}}}"#)).reason
}

#[test]
fn a_field_with_a_vector_and_k_parses() {
    let k = knn(r#"{"knn":{"v":{"vector":[1,"2.5",0],"k":"3","boost":2}}}"#);
    assert_eq!(k.field, "v");
    assert_eq!(k.vector, vec![1.0, 2.5, 0.0]);
    assert_eq!((k.mode, k.boost), (knn::Mode::TopK(3), 2.0));
    assert_eq!(k.filter, None);
}

#[test]
fn k_has_a_range_of_one_to_ten_thousand() {
    let ok = |k: &str| knn(&format!(r#"{{"knn":{{"v":{{"vector":[1],"k":{k}}}}}}}"#)).mode;
    assert_eq!(
        (ok("1"), ok("10000")),
        (knn::Mode::TopK(1), knn::Mode::TopK(10_000))
    );
    for k in ["0", "-1", "10001"] {
        assert_eq!(
            reason(&format!(r#""vector":[1],"k":{k}"#)),
            "[knn] requires k to be in the range (0, 10000]"
        );
    }
}

#[test]
fn exactly_one_of_k_and_the_radial_forms_is_asked_for_before_the_vector() {
    let one = "[knn] requires exactly one of k, distance or score to be set";
    assert_eq!(reason(r#""vector":[1]"#), one);
    assert_eq!(reason(r#""vector":[1],"k":2,"min_score":1"#), one);
    assert_eq!(
        reason(r#""vector":[1],"min_score":1,"max_distance":1"#),
        one
    );
    assert_eq!(reason(r#""k":0,"min_score":1"#), one);
    assert_eq!(reason(r#""k":2"#), "[knn] requires query vector");
}

#[test]
fn a_radial_form_alone_is_a_score_or_a_distance() {
    let mode = |param: &str| knn(&format!(r#"{{"knn":{{"v":{{"vector":[1],{param}}}}}}}"#)).mode;
    assert_eq!(mode(r#""min_score":0.5"#), knn::Mode::MinScore(0.5));
    assert_eq!(mode(r#""max_distance":"2""#), knn::Mode::MaxDistance(2.0));
    // A distance may be zero or negative here: the space decides when it is built.
    assert_eq!(mode(r#""max_distance":-1"#), knn::Mode::MaxDistance(-1.0));
    for score in ["0", "-1"] {
        assert_eq!(
            reason(&format!(r#""vector":[1],"min_score":{score}"#)),
            "[knn] requires minScore to be greater than 0"
        );
    }
    assert_eq!(
        reason(r#""vector":[1],"min_score":"x""#),
        "[1:48] [knn] failed to parse field [min_score]"
    );
    assert_eq!(
        reason(r#""vector":[1],"max_distance":[1]"#),
        "[1:51] [knn] max_distance doesn't support values of type: START_ARRAY"
    );
    assert_eq!(
        reason(r#""vector":[1],"min_score":1,"min_score":2"#),
        "a repeated [knn] parameter [min_score] is not supported by this node yet"
    );
    assert_eq!(
        reason(r#""vector":[1],"max_distance":1,"max_distance":2"#),
        "a repeated [knn] parameter [max_distance] is not supported by this node yet"
    );
}

#[test]
fn the_parameters_that_change_the_walk_are_refused_not_ignored() {
    for param in [
        r#""method_parameters":{"ef_search":10}"#,
        r#""rescore":true"#,
        r#""expand_nested_docs":true"#,
        r#""ignore_unmapped":true"#,
    ] {
        let r = reason(&format!(r#""vector":[1],"k":1,{param}"#));
        assert!(r.ends_with("is not supported by this node yet"), "{r}");
    }
}

#[test]
fn a_repeated_parameter_and_a_negative_boost_are_refused() {
    assert_eq!(
        reason(r#""vector":[1],"k":1,"k":2"#),
        "a repeated [knn] parameter [k] is not supported by this node yet"
    );
    assert_eq!(
        reason(r#""vector":[1],"vector":[2],"k":2"#),
        "a repeated [knn] parameter [vector] is not supported by this node yet"
    );
    assert_eq!(
        reason(r#""vector":[1],"k":1,"filter":{"match_all":{}},"filter":{"match_all":{}}"#),
        "a repeated [knn] parameter [filter] is not supported by this node yet"
    );
    assert_eq!(
        reason(r#""vector":[1],"k":1,"boost":1,"boost":2"#),
        "a repeated [knn] parameter [boost] is not supported by this node yet"
    );
    assert_eq!(
        reason(r#""vector":[1],"k":1,"boost":-1"#),
        "a negative [knn] boost is not supported by this node yet"
    );
    assert_eq!(
        knn(r#"{"knn":{"v":{"vector":[1],"k":1,"boost":0}}}"#).boost,
        0.0
    );
}

#[test]
fn a_vector_that_is_not_numbers_fails_as_the_plugin_does() {
    assert_eq!(
        reason(r#""vector":[1,"x"],"k":1"#),
        "[1:35] [knn] failed to parse field [vector]"
    );
    assert_eq!(
        reason(r#""vector":[1,[2]],"k":1"#),
        "[1:35] [knn] failed to parse field [vector]"
    );
    let nested = failure(r#"{"knn":{"v":{"vector":[1,[2]],"k":1}}}"#);
    assert_eq!(
        nested.cause.as_deref().map(|c| c.reason.as_str()),
        Some("expected value but got [START_ARRAY]")
    );
    assert!(reason(r#""vector":[1,null],"k":1"#).ends_with("is not supported by this node yet"));
    assert!(reason(r#""vector":5,"k":1"#).ends_with("is not supported by this node yet"));
}

#[test]
fn a_filter_is_a_query_and_nothing_else() {
    let k = knn(r#"{"knn":{"v":{"vector":[1],"k":1,"filter":{"term":{"tag":"a"}}}}}"#);
    assert!(matches!(k.filter, Some(Query::Term { .. })), "{k:?}");
    assert_eq!(
        reason(r#""vector":[1],"k":1,"filter":5"#),
        "[1:51] [knn] filter doesn't support values of type: VALUE_NUMBER"
    );
    assert_eq!(
        reason(r#""vector":[1],"k":1,"filter":{"nope":{}}"#),
        "[1:59] [knn] failed to parse field [filter]"
    );
}

#[test]
fn parameters_of_the_wrong_type_name_the_token() {
    assert_eq!(
        reason(r#""vector":[1],"k":[2]"#),
        "[1:40] [knn] k doesn't support values of type: START_ARRAY"
    );
    assert_eq!(
        reason(r#""vector":[1],"k":1,"boost":null"#),
        "[1:50] [knn] boost doesn't support values of type: VALUE_NULL"
    );
    assert_eq!(
        reason(r#""vector":[1],"k":1,"boost":"x""#),
        "[1:50] [knn] failed to parse field [boost]"
    );
    assert_eq!(
        reason(r#""vector":[1],"k":"x""#),
        "[1:40] [knn] failed to parse field [k]"
    );
    assert_eq!(
        reason(r#""vector":[1],"k":1,"bogus":1"#),
        "[1:42] [knn] unknown field [bogus]"
    );
}

#[test]
fn the_query_names_one_field() {
    assert_eq!(
        failure(r#"{"knn":{"v":{"vector":[1],"k":1},"w":{"vector":[1],"k":1}}}"#).reason,
        "[knn] query doesn't support multiple fields, found [v] and [w]"
    );
    assert_eq!(
        failure(r#"{"knn":{}}"#).reason,
        "[knn] field 'vector' requires to be non-null and non-empty"
    );
    // A field with no object holds no parameters.
    assert_eq!(
        failure(r#"{"knn":{"v":[1,0,0]}}"#).reason,
        "[knn] requires exactly one of k, distance or score to be set"
    );
}

#[test]
fn the_nodes_of_a_query_are_found_through_its_compound_queries() {
    let node = |inner: &str| format!(r#"{{"knn":{{"v":{inner}}}}}"#);
    let n = node(r#"{"vector":[1],"k":1}"#);
    let body = format!(
        r#"{{"query":{{"bool":{{"must":[{n}],"should":[{n}],"must_not":[{n}],"filter":[{n},
        {{"constant_score":{{"filter":{n}}}}},{{"dis_max":{{"queries":[{n},{n}]}}}},
        {{"function_score":{{"query":{n}}}}},{{"function_score":{{"functions":[]}}}},
        {{"match_all":{{}}}},{{"ids":{{"values":["a"]}}}}]}}}}}}"#
    );
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let found = crate::knn::nodes(source.query.as_ref().expect("query"));
    assert_eq!(found.len(), 8);
    let top = parse_chunks([br#"{"query":{"match_all":{}}}"#.as_slice()]).expect("parses");
    assert!(crate::knn::nodes(top.query.as_ref().expect("query")).is_empty());
}

#[test]
fn knn_is_among_the_registered_names() {
    assert!(NAMES.contains(&"knn"));
    assert!(crate::dsl::query::REGISTERED_QUERIES.contains(&"knn"));
}
