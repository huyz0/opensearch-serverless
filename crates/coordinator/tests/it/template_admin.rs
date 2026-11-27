//! What the template admin routes decide (docs/specs/template-admin/SPEC.md).

use oss_coordinator::template_admin::{Kind, concrete_name, render, select, validate};

fn refused(kind: Kind, body: &str) -> (u16, &'static str) {
    let r = validate(kind, body.as_bytes()).expect_err("refused");
    (r.status, r.kind)
}

#[test]
fn an_index_template_needs_patterns_and_a_component_must_not_have_them() {
    assert!(validate(Kind::Index, br#"{"index_patterns":["a-*"]}"#).is_ok());
    assert_eq!(
        refused(Kind::Index, r#"{"template":{}}"#),
        (400, "illegal_argument_exception")
    );
    assert_eq!(
        refused(Kind::Index, r#"{"index_patterns":null}"#),
        (400, "illegal_argument_exception")
    );
    assert!(validate(Kind::Component, br#"{"template":{}}"#).is_ok());
    assert_eq!(
        refused(Kind::Component, r#"{"index_patterns":["a"]}"#),
        (400, "illegal_argument_exception")
    );
}

#[test]
fn a_body_that_is_not_one_object_is_malformed() {
    for body in ["", "[]", "{", "7"] {
        assert_eq!(
            refused(Kind::Component, body),
            (400, "malformed_body"),
            "{body}"
        );
    }
}

#[test]
fn aliases_replicas_and_bad_shapes_are_refused_not_stored() {
    let index = |t: &str| format!(r#"{{"index_patterns":["a"],{t}}}"#);
    assert_eq!(
        refused(Kind::Index, &index(r#""template":{"aliases":{"x":{}}}"#)),
        (501, "unsupported_template")
    );
    assert!(
        validate(
            Kind::Index,
            index(r#""template":{"aliases":{}}"#).as_bytes()
        )
        .is_ok()
    );
    assert_eq!(
        refused(Kind::Index, &index(r#""data_stream":[]"#)),
        (400, "illegal_argument_exception")
    );
    assert!(validate(Kind::Index, index(r#""data_stream":{}"#).as_bytes()).is_ok());
    assert_eq!(
        refused(Kind::Index, &index(r#""template":{"settings":3}"#)),
        (400, "malformed_body")
    );
    for replicas in [
        r#""template":{"settings":{"number_of_replicas":1}}"#,
        r#""template":{"settings":{"index":{"number_of_replicas":"2"}}}"#,
        r#""template":{"settings":{"index.number_of_replicas":1}}"#,
    ] {
        assert_eq!(
            refused(Kind::Index, &index(replicas)),
            (501, "unsupported_setting")
        );
    }
    assert!(
        validate(
            Kind::Index,
            index(r#""template":{"settings":{"number_of_replicas":0,"number_of_shards":3}}"#)
                .as_bytes()
        )
        .is_ok()
    );
}

#[test]
fn a_mutation_names_one_concrete_template() {
    assert!(concrete_name("logs").is_ok());
    for name in ["_simulate", "a*", "a?b", "", "  "] {
        assert_eq!(
            concrete_name(name).expect_err("refused").status,
            501,
            "{name}"
        );
    }
}

#[test]
fn a_name_selects_one_a_prefix_or_all() {
    let docs: Vec<(&str, &[u8])> = vec![("a", b"{}"), ("ab", b"{}"), ("b", b"{}")];
    let pick = |name| {
        select(docs.iter().copied(), name).map(|v| v.iter().map(|(n, _)| *n).collect::<Vec<_>>())
    };
    assert_eq!(pick(None).unwrap(), ["a", "ab", "b"]);
    assert_eq!(pick(Some("a*")).unwrap(), ["a", "ab"]);
    assert_eq!(pick(Some("zz*")).unwrap(), Vec::<&str>::new());
    assert_eq!(pick(Some("b")).unwrap(), ["b"]);
    let missing = pick(Some("c")).unwrap_err();
    assert_eq!((missing.status, missing.kind), (404, "template_missing"));
    assert_eq!(pick(Some("_x")).unwrap_err().status, 501);
}

#[test]
fn a_get_answers_the_name_and_the_definition_under_the_sets_key() {
    let found: Vec<(&str, &[u8])> = vec![("a\"b", br#"{"index_patterns":["x"]}"#)];
    assert_eq!(
        render(Kind::Index, &found),
        r#"{"index_templates":[{"name":"a\"b","index_template":{"index_patterns":["x"]}}]}"#
    );
    assert_eq!(
        render(Kind::Component, &[("c", b"{}"), ("d", b"{}")]),
        r#"{"component_templates":[{"name":"c","component_template":{}},{"name":"d","component_template":{}}]}"#
    );
    assert_eq!(render(Kind::Index, &[]), r#"{"index_templates":[]}"#);
}
