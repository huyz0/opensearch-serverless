//! What the ingest pipeline admin routes decide (docs/specs/pipeline-routes/SPEC.md).

use oss_coordinator::pipeline_admin::{SUPPORTED, concrete_id, render, select, validate};

fn refused(body: &str) -> (u16, &'static str, String) {
    let r = validate(body.as_bytes()).expect_err("refused");
    (r.status, r.kind, r.reason)
}

#[test]
fn a_pipeline_of_supported_processors_is_accepted() {
    let configs = [
        ("append", r#"{"field":"a","value":1}"#),
        ("convert", r#"{"field":"a","type":"integer"}"#),
        ("lowercase", r#"{"field":"a"}"#),
        ("remove", r#"{"field":"a"}"#),
        ("rename", r#"{"field":"a","target_field":"b"}"#),
        ("set", r#"{"field":"a","value":1}"#),
        ("trim", r#"{"field":"a"}"#),
        ("uppercase", r#"{"field":"a"}"#),
    ];
    assert_eq!(configs.len(), SUPPORTED.len());
    for (name, config) in configs {
        let body = format!(r#"{{"processors":[{{"{name}":{config}}}]}}"#);
        assert!(validate(body.as_bytes()).is_ok(), "{name}");
    }
    assert!(validate(br#"{"description":"d","version":2,"processors":[]}"#).is_ok());
}

#[test]
fn an_unsupported_processor_is_refused_with_the_available_list() {
    let (status, kind, reason) = refused(r#"{"processors":[{"script":{"source":"x"}}]}"#);
    assert_eq!((status, kind), (400, "invalid_pipeline"));
    assert!(
        reason.contains("[script]") && reason.contains("Available processors: append,"),
        "{reason}"
    );
    let (_, _, nested) =
        refused(r#"{"processors":[{"set":{"field":"a","value":1,"on_failure":[{"grok":{}}]}}]}"#);
    assert!(nested.contains("[grok]"), "{nested}");
    let (_, _, top) = refused(r#"{"processors":[],"on_failure":[{"geoip":{}}]}"#);
    assert!(top.contains("[geoip]"), "{top}");
}

#[test]
fn a_malformed_pipeline_is_refused() {
    assert_eq!(refused("[]").1, "malformed_body");
    assert_eq!(refused("{").1, "malformed_body");
    for body in [
        r#"{"description":"x"}"#,
        r#"{"processors":{}}"#,
        r#"{"processors":[1]}"#,
        r#"{"processors":[{}]}"#,
        r#"{"processors":[{"set":{},"remove":{}}]}"#,
        r#"{"processors":[{"set":{"field":"a"}}]}"#,
        r#"{"processors":[{"set":{"field":"a","value":"{{b}}"}}]}"#,
        r#"{"processors":[{"set":{"field":"a","value":1,"if":"true"}}]}"#,
        r#"{"processors":[{"convert":{"field":"a","type":"ip"}}]}"#,
    ] {
        assert_eq!(refused(body).1, "invalid_pipeline", "{body}");
    }
}

#[test]
fn on_failure_nesting_is_bounded() {
    let mut body = String::from(r#"{"set":{"field":"a","value":1}}"#);
    for _ in 0..9 {
        body = format!(r#"{{"set":{{"field":"a","value":1,"on_failure":[{body}]}}}}"#);
    }
    let deep = format!(r#"{{"processors":[{body}]}}"#);
    assert!(refused(&deep).2.contains("nested too deeply"));
}

#[test]
fn a_mutation_names_one_concrete_id() {
    assert!(concrete_id("p1").is_ok());
    for id in ["_simulate", "a*", "a?", "a,b", "", " "] {
        assert_eq!(concrete_id(id).expect_err("refused").status, 501, "{id}");
    }
}

#[test]
fn an_id_selects_one_or_all() {
    let docs: Vec<(&str, &[u8])> = vec![("a", b"{}"), ("b", b"{}")];
    let pick = |id| select(docs.iter().copied(), id).map(|v| v.len());
    assert_eq!(pick(None).unwrap(), 2);
    assert_eq!(pick(Some("*")).unwrap(), 2);
    assert_eq!(pick(Some("_all")).unwrap(), 2);
    assert_eq!(pick(Some("a")).unwrap(), 1);
    let missing = select(docs.iter().copied(), Some("c")).unwrap_err();
    assert_eq!((missing.status, missing.kind), (404, "pipeline_missing"));
    for id in ["a,b", "a*", "_x"] {
        assert_eq!(
            select(docs.iter().copied(), Some(id)).unwrap_err().status,
            501,
            "{id}"
        );
    }
}

#[test]
fn a_get_answers_an_object_of_ids() {
    assert_eq!(render(&[]), "{}");
    assert_eq!(
        render(&[("a\"", br#"{"processors":[]}"#), ("b", b"{}")]),
        r#"{"a\"":{"processors":[]},"b":{}}"#
    );
}
