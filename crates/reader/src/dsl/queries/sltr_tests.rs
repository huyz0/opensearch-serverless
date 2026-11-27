//! Tests of the `sltr` parser; compiled only under `#[cfg(test)]`.

use super::*;
use crate::dsl::parse_chunks;
use crate::template::Val;

fn parse(body: &str) -> Result<sltr::Sltr, String> {
    match parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]) {
        Ok(s) => match s.query {
            Some(Query::More(m)) => match *m {
                More::Sltr(s) => Ok(*s),
                other => panic!("parsed {other:?}"),
            },
            other => panic!("parsed {other:?}"),
        },
        Err(e) => Err(e.into_failure().reason),
    }
}

#[test]
fn a_model_with_params_and_active_features_parses() {
    let s = parse(
        r#"{"sltr":{"model":"m","store":"s","params":{"q":"x"},"active_features":["a","b"]}}"#,
    )
    .expect("parses");
    assert_eq!(s.model, "m");
    assert_eq!(s.store.as_deref(), Some("s"));
    assert_eq!(
        s.active_features,
        Some(vec!["a".to_owned(), "b".to_owned()])
    );
    assert!(matches!(s.params, Val::Map(ref m) if m.len() == 1));
}

#[test]
fn only_the_model_is_required() {
    let s = parse(r#"{"sltr":{"model":"m"}}"#).expect("parses");
    assert_eq!((s.store, s.active_features), (None, None));
    assert_eq!(s.params, Val::Map(Vec::new()));
}

#[test]
fn malformed_bodies_are_refused_by_name() {
    for (body, want) in [
        (r#"{"sltr":{}}"#, "needs one of a [model] or a [featureset]"),
        (
            r#"{"sltr":{"model":"m","featureset":"f"}}"#,
            "needs one of a [model] or a [featureset]",
        ),
        (r#"{"sltr":{"model":1}}"#, "[model] must be a string"),
        (
            r#"{"sltr":{"model":"m","store":1}}"#,
            "[store] must be a string",
        ),
        (
            r#"{"sltr":{"model":"m","params":3}}"#,
            "[params] must be an object",
        ),
        (
            r#"{"sltr":{"model":"m","active_features":"a"}}"#,
            "[active_features] must be an array",
        ),
        (
            r#"{"sltr":{"model":"m","active_features":[1]}}"#,
            "[active_features] holds strings",
        ),
        (r#"{"sltr":{"model":"m","zzz":1}}"#, "unknown field [zzz]"),
    ] {
        let reason = parse(body).expect_err(body);
        assert!(reason.contains(want), "{body}: {reason}");
    }
}

#[test]
fn a_logging_query_names_a_feature_set_and_itself() {
    let s = parse(r#"{"sltr":{"_name":"n","featureset":"f","params":{"q":"x"}}}"#).expect("parses");
    assert_eq!(s.featureset.as_deref(), Some("f"));
    assert_eq!(s.name.as_deref(), Some("n"));
    assert_eq!(s.model, "");
}
