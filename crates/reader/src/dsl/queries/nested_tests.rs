//! Tests of the `nested` parser; compiled only under `#[cfg(test)]`.

use super::nested::{NestedQuery, ScoreMode};
use super::*;
use crate::dsl::parse_chunks;

fn parsed(body: &str) -> Result<More, crate::failure::Failure> {
    match parse_chunks([body.as_bytes()]) {
        Ok(s) => match s.query {
            Some(Query::More(m)) => Ok(*m),
            other => panic!("parsed {other:?}"),
        },
        Err(e) => Err(e.into_failure()),
    }
}

fn reason(body: &str) -> String {
    parsed(body).expect_err("fails").reason
}

#[test]
fn a_nested_query_holds_its_path_query_and_options() {
    let More::Nested(q, boost) = parsed(
        r#"{"query":{"nested":{"path":"c","query":{"term":{"c.w":"a"}},"score_mode":"sum","ignore_unmapped":true,"boost":2}}}"#,
    )
    .expect("parses") else {
        panic!("not nested");
    };
    assert_eq!(boost, 2.0);
    let NestedQuery {
        path,
        score_mode,
        ignore_unmapped,
        ..
    } = *q;
    assert_eq!(
        (path.as_str(), score_mode, ignore_unmapped),
        ("c", ScoreMode::Sum, true)
    );
}

#[test]
fn the_score_mode_defaults_to_avg_and_names_each_mode() {
    let mode = |extra: &str| {
        let body = format!(
            r#"{{"query":{{"nested":{{"path":"c","query":{{"match_all":{{}}}}{extra}}}}}}}"#
        );
        match parsed(&body) {
            Ok(More::Nested(q, _)) => Ok(q.score_mode),
            Ok(other) => panic!("{other:?}"),
            Err(f) => Err(f.reason),
        }
    };
    assert_eq!(mode(""), Ok(ScoreMode::Avg));
    for (name, want) in [
        ("none", ScoreMode::None),
        ("min", ScoreMode::Min),
        ("max", ScoreMode::Max),
        ("avg", ScoreMode::Avg),
        ("sum", ScoreMode::Sum),
    ] {
        assert_eq!(mode(&format!(r#","score_mode":"{name}""#)), Ok(want));
    }
    assert_eq!(
        mode(r#","score_mode":"total""#),
        Err("No score mode for child query [total] found".to_owned())
    );
}

#[test]
fn a_nested_query_without_a_path_or_query_is_refused() {
    assert_eq!(
        reason(r#"{"query":{"nested":{"query":{"match_all":{}}}}}"#),
        "[nested] requires 'path' field"
    );
    assert_eq!(
        reason(r#"{"query":{"nested":{"path":"c"}}}"#),
        "[nested] requires 'query' field"
    );
}

#[test]
fn what_is_not_served_is_refused_by_name() {
    assert_eq!(
        reason(r#"{"query":{"nested":{"path":"c","query":{"match_all":{}},"nope":1}}}"#),
        "[nested] query does not support [nope]"
    );
}
