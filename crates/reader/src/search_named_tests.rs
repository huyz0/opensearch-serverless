//! Tests of collecting the named queries of a search; compiled only under `#[cfg(test)]`.

use super::*;
use crate::dsl::parse_chunks;

fn names(body: &str) -> Vec<String> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let q = source.query.expect("a query");
    collect(&q).into_iter().map(|(n, _)| n.to_owned()).collect()
}

#[test]
fn names_come_in_request_order_through_every_bool_list_and_wrapper() {
    let body = r#"{"query":{"bool":{
        "_name":"top",
        "must":[{"term":{"a":{"value":"x","_name":"m"}}}],
        "should":[{"constant_score":{"filter":{"exists":{"field":"f","_name":"s"}}}}],
        "filter":[{"ids":{"values":["1"],"_name":"f"}}],
        "must_not":[{"match_all":{"_name":"n"}}]
    }}}"#;
    // `constant_score` takes no name yet, so only the wrapped query's is seen.
    assert_eq!(names(body), ["top", "m", "s", "f", "n"]);
}

#[test]
fn a_name_used_twice_keeps_its_first_query_and_no_name_is_none() {
    let twice = r#"{"query":{"bool":{"should":[
        {"term":{"a":{"value":"x","_name":"same"}}},
        {"term":{"b":{"value":"y","_name":"same"}}}
    ]}}}"#;
    assert_eq!(names(twice), ["same"]);
    assert!(names(r#"{"query":{"match_all":{}}}"#).is_empty());
}
