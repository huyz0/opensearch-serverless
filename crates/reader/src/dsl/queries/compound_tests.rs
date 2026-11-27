//! Tests of the compound parsers; compiled only under `#[cfg(test)]`.

use super::*;
use crate::dsl::parse_chunks;
use crate::failure::Failure;

fn more(body: &str) -> More {
    match parse_chunks([body.as_bytes()]).map(|s| s.query) {
        Ok(Some(Query::More(m))) => *m,
        other => panic!("parsed {other:?}"),
    }
}

fn failure(body: &str) -> Failure {
    match parse_chunks([body.as_bytes()]) {
        Err(e) => e.into_failure(),
        Ok(s) => panic!("parsed {s:?}"),
    }
}

fn reason(body: &str) -> String {
    failure(body).reason
}

fn q(inner: &str) -> String {
    format!(r#"{{"query":{inner}}}"#)
}

fn s(text: &str) -> Value {
    Value::Str(text.into())
}

#[test]
fn compound_queries_hold_their_inner_queries() {
    let tag = || Query::Term {
        field: "tag".into(),
        value: s("a"),
        boost: 1.0,
    };
    assert_eq!(
        more(&q(
            r#"{"constant_score":{"filter":{"term":{"tag":"a"}},"boost":2}}"#
        )),
        More::ConstantScore {
            filter: tag(),
            boost: 2.0
        }
    );
    assert_eq!(
        more(&q(
            r#"{"dis_max":{"queries":[{"term":{"tag":"a"}},{"term":{"tag":"a"}}],"tie_breaker":0.5,"boost":2}}"#
        )),
        More::DisMax {
            queries: vec![tag(), tag()],
            tie_breaker: 0.5,
            boost: 2.0
        }
    );
    assert_eq!(
        more(&q(r#"{"dis_max":{"queries":{"term":{"tag":"a"}}}}"#)),
        More::DisMax {
            queries: vec![tag()],
            tie_breaker: 0.0,
            boost: 1.0
        }
    );
    assert_eq!(
        more(&q(r#"{"match_none":{"boost":2}}"#)),
        More::MatchNone { boost: 2.0 }
    );
}

#[test]
fn compound_queries_refuse_what_opensearch_refuses() {
    assert_eq!(
        reason(&q(r#"{"constant_score":{"boost":2}}"#)),
        "[constant_score] requires a 'filter' element"
    );
    assert_eq!(
        reason(&q(r#"{"constant_score":{"query":{"match_all":{}}}}"#)),
        "[constant_score] query does not support [query]"
    );
    assert_eq!(
        reason(&q(r#"{"constant_score":{"x":1}}"#)),
        "[constant_score] query does not support [x]"
    );
    assert_eq!(
        reason(&q(r#"{"constant_score":{"filter":[]}}"#)),
        "unexpected token [START_ARRAY]"
    );
    assert_eq!(
        reason(&q(r#"{"dis_max":{"tie_breaker":0.5}}"#)),
        "[dis_max] requires 'queries' field with at least one clause"
    );
    assert_eq!(
        reason(&q(r#"{"dis_max":{"queries":[]}}"#)),
        "[_na] query malformed, must start with start_object"
    );
    assert_eq!(
        reason(&q(r#"{"dis_max":{"query":[]}}"#)),
        "[dis_max] query does not support [query]"
    );
    assert_eq!(
        reason(&q(r#"{"dis_max":{"x":1}}"#)),
        "[dis_max] query does not support [x]"
    );
    assert_eq!(
        reason(&q(r#"{"dis_max":{"query":{"match_all":{}}}}"#)),
        "[dis_max] query does not support [query]"
    );
    assert_eq!(
        reason(&q(r#"{"match_none":{"x":1}}"#)),
        "[match_none] query does not support [x]"
    );
    assert_eq!(
        reason(&q(r#"{"match_none":{"x":{}}}"#)),
        "[match_none] unknown token [START_OBJECT] after [x]"
    );
}

#[test]
fn dis_max_holds_at_most_the_clause_limit() {
    let one = r#"{"match_all":{}}"#;
    let many = vec![one; crate::dsl::bool_query::MAX_CLAUSES + 1].join(",");
    assert_eq!(
        reason(&q(&format!(r#"{{"dis_max":{{"queries":[{many}]}}}}"#))),
        "too many clauses, the maximum is [1024]"
    );
}

#[test]
fn boosting_holds_both_queries_and_its_factors() {
    let tag = |v: &str| Query::Term {
        field: "tag".into(),
        value: s(v),
        boost: 1.0,
    };
    assert_eq!(
        more(&q(
            r#"{"boosting":{"positive":{"term":{"tag":"a"}},"negative":{"term":{"tag":"b"}},"negative_boost":0.5,"boost":2}}"#
        )),
        More::Boosting {
            positive: tag("a"),
            negative: tag("b"),
            negative_boost: 0.5,
            boost: 2.0,
        }
    );
}

#[test]
fn boosting_fails_as_the_builder_fails() {
    let body = |inner: &str| q(&format!(r#"{{"boosting":{inner}}}"#));
    let both = r#""positive":{"match_all":{}},"negative":{"match_all":{}}"#;
    assert_eq!(
        reason(&body(&format!(r#"{{{both}}}"#))),
        "[boosting] query requires 'negative_boost' to be set to be a positive value'"
    );
    assert_eq!(
        reason(&body(r#"{"negative":{"match_all":{}},"negative_boost":1}"#)),
        "[boosting] query requires 'positive' query to be set'"
    );
    assert_eq!(
        reason(&body(r#"{"positive":{"match_all":{}},"negative_boost":1}"#)),
        "[boosting] query requires 'negative' query to be set'"
    );
    assert_eq!(
        reason(&body(&format!(r#"{{{both},"negative_boost":-1}}"#))),
        "[boosting] query requires 'negative_boost' to be set to be a positive value'"
    );
    assert_eq!(
        reason(&body(&format!(r#"{{{both},"negative_boost":1,"x":1}}"#))),
        "[boosting] query does not support [x]"
    );
}
