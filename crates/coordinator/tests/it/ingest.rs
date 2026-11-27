//! The ingest pipeline engine (docs/specs/ingest-pipelines/SPEC.md).

use oss_coordinator::ingest::Pipeline;

fn run(pipeline: &str, source: &str) -> Result<String, String> {
    let p = Pipeline::compile(pipeline.as_bytes()).expect("compiles");
    p.run(source.as_bytes())
        .map(|b| String::from_utf8(b).expect("utf8"))
}

fn one(processor: &str, source: &str) -> Result<String, String> {
    run(&format!(r#"{{"processors":[{processor}]}}"#), source)
}

#[test]
fn set_writes_a_field_and_honours_override_and_ignore_empty_value() {
    let set = |extra: &str| format!(r#"{{"set":{{"field":"a.b","value":"x"{extra}}}}}"#);
    assert_eq!(
        one(&set(""), r#"{"n":1}"#).unwrap(),
        r#"{"n":1,"a":{"b":"x"}}"#
    );
    assert_eq!(
        one(&set(""), r#"{"a":{"b":"old"}}"#).unwrap(),
        r#"{"a":{"b":"x"}}"#
    );
    assert_eq!(
        one(&set(r#","override":false"#), r#"{"a":{"b":"old"}}"#).unwrap(),
        r#"{"a":{"b":"old"}}"#
    );
    let empty = r#"{"set":{"field":"a","value":"","ignore_empty_value":true}}"#;
    assert_eq!(one(empty, "{}").unwrap(), "{}");
    assert!(
        one(&set(""), r#"{"a":5}"#).is_err(),
        "a scalar is not an object"
    );
}

#[test]
fn remove_and_rename_move_fields_and_report_missing_ones() {
    assert_eq!(
        one(
            r#"{"remove":{"field":["a","b.c"]}}"#,
            r#"{"a":1,"b":{"c":2,"d":3}}"#
        )
        .unwrap(),
        r#"{"b":{"d":3}}"#
    );
    let missing = one(r#"{"remove":{"field":"z"}}"#, "{}").unwrap_err();
    assert_eq!(missing, "field [z] not present as part of path [z]");
    assert_eq!(
        one(r#"{"remove":{"field":"z","ignore_missing":true}}"#, "{}").unwrap(),
        "{}"
    );
    let rename = r#"{"rename":{"field":"a","target_field":"x.y"}}"#;
    assert_eq!(one(rename, r#"{"a":1}"#).unwrap(), r#"{"x":{"y":1}}"#);
    assert_eq!(
        one(
            r#"{"rename":{"field":"a","target_field":"b"}}"#,
            r#"{"a":1,"b":2}"#
        )
        .unwrap_err(),
        "field [b] already exists"
    );
    assert!(one(rename, "{}").is_err());
    assert_eq!(
        one(
            r#"{"rename":{"field":"a","target_field":"b","ignore_missing":true}}"#,
            "{}"
        )
        .unwrap(),
        "{}"
    );
}

#[test]
fn text_processors_change_case_and_trim_into_a_target() {
    assert_eq!(
        one(r#"{"lowercase":{"field":"a"}}"#, r#"{"a":"AbC"}"#).unwrap(),
        r#"{"a":"abc"}"#
    );
    assert_eq!(
        one(
            r#"{"uppercase":{"field":"a","target_field":"b"}}"#,
            r#"{"a":"abc"}"#
        )
        .unwrap(),
        r#"{"a":"abc","b":"ABC"}"#
    );
    assert_eq!(
        one(r#"{"trim":{"field":"a"}}"#, r#"{"a":"  x "}"#).unwrap(),
        r#"{"a":"x"}"#
    );
    assert_eq!(
        one(r#"{"trim":{"field":"a"}}"#, r#"{"a":3}"#).unwrap_err(),
        "field [a] of type [java.lang.Integer] cannot be cast to [java.lang.String]"
    );
    assert!(one(r#"{"trim":{"field":"a"}}"#, "{}").is_err());
    assert_eq!(
        one(r#"{"trim":{"field":"a","ignore_missing":true}}"#, "{}").unwrap(),
        "{}"
    );
}

#[test]
fn append_builds_a_list_and_can_skip_duplicates() {
    let append = |extra: &str| format!(r#"{{"append":{{"field":"t","value":["a","b"]{extra}}}}}"#);
    assert_eq!(one(&append(""), "{}").unwrap(), r#"{"t":["a","b"]}"#);
    assert_eq!(
        one(&append(""), r#"{"t":"a"}"#).unwrap(),
        r#"{"t":["a","a","b"]}"#
    );
    assert_eq!(
        one(&append(r#","allow_duplicates":false"#), r#"{"t":["a"]}"#).unwrap(),
        r#"{"t":["a","b"]}"#
    );
    assert_eq!(
        one(r#"{"append":{"field":"t","value":7}}"#, r#"{"t":[1]}"#).unwrap(),
        r#"{"t":[1,7]}"#
    );
}

#[test]
fn convert_changes_types_and_refuses_what_does_not_parse() {
    let convert = |ty: &str| format!(r#"{{"convert":{{"field":"a","type":"{ty}"}}}}"#);
    assert_eq!(
        one(&convert("integer"), r#"{"a":" 42 "}"#).unwrap(),
        r#"{"a":42}"#
    );
    assert_eq!(
        one(&convert("long"), r#"{"a":"9000000000"}"#).unwrap(),
        r#"{"a":9000000000}"#
    );
    assert!(one(&convert("integer"), r#"{"a":"9000000000"}"#).is_err());
    assert_eq!(
        one(&convert("double"), r#"{"a":"1.5"}"#).unwrap(),
        r#"{"a":1.5}"#
    );
    assert_eq!(
        one(&convert("float"), r#"{"a":2}"#).unwrap(),
        r#"{"a":2.0}"#
    );
    assert_eq!(
        one(&convert("boolean"), r#"{"a":"true"}"#).unwrap(),
        r#"{"a":true}"#
    );
    assert!(one(&convert("boolean"), r#"{"a":"yes"}"#).is_err());
    assert_eq!(
        one(&convert("string"), r#"{"a":7}"#).unwrap(),
        r#"{"a":"7"}"#
    );
    assert_eq!(
        one(&convert("integer"), r#"{"a":["1","2"]}"#).unwrap(),
        r#"{"a":[1,2]}"#
    );
    assert!(one(&convert("integer"), r#"{"a":{}}"#).is_err());
    assert!(one(&convert("integer"), "{}").is_err());
    let into = r#"{"convert":{"field":"a","target_field":"b","type":"integer"}}"#;
    assert_eq!(one(into, r#"{"a":"1"}"#).unwrap(), r#"{"a":"1","b":1}"#);
}

#[test]
fn failures_are_taken_by_on_failure_or_ignore_failure() {
    let p = r#"{"processors":[{"remove":{"field":"z","ignore_failure":true}},{"set":{"field":"a","value":1}}]}"#;
    assert_eq!(run(p, "{}").unwrap(), r#"{"a":1}"#);
    let handled = r#"{"processors":[{"remove":{"field":"z","on_failure":[{"set":{"field":"err","value":"x"}}]}}]}"#;
    assert_eq!(run(handled, "{}").unwrap(), r#"{"err":"x"}"#);
    let top = r#"{"processors":[{"remove":{"field":"z"}}],"on_failure":[{"set":{"field":"err","value":"y"}}]}"#;
    assert_eq!(run(top, "{}").unwrap(), r#"{"err":"y"}"#);
    let failing = r#"{"processors":[{"remove":{"field":"z"}}]}"#;
    assert!(run(failing, "{}").is_err());
    assert!(run(failing, "[]").is_err());
    assert!(run(failing, "{").is_err());
}

#[test]
fn processors_run_in_order() {
    let p = r#"{"processors":[{"set":{"field":"a","value":"  Hi "}},{"trim":{"field":"a"}},{"uppercase":{"field":"a"}}]}"#;
    assert_eq!(run(p, "{}").unwrap(), r#"{"a":"HI"}"#);
}

#[test]
fn what_the_engine_does_not_run_is_refused_at_compile() {
    for bad in [
        "[]",
        "{}",
        r#"{"processors":{}}"#,
        r#"{"processors":[{"set":{"field":"a","value":1,"copy_from":"b"}}]}"#,
        r#"{"processors":[{"remove":{"field":[]}}]}"#,
        r#"{"processors":[{"remove":{"field":[1]}}]}"#,
        r#"{"processors":[{"set":{"field":"a","value":1,"override":"no"}}]}"#,
        r#"{"processors":[{"convert":{"field":"a","type":"auto"}}]}"#,
        r#"{"processors":[{"append":{"field":"a","value":["{{x}}"]}}]}"#,
        r#"{"processors":[{"set":{"field":"a","value":{"k":"{{x}}"}}}]}"#,
        r#"{"processors":[],"on_failure":{}}"#,
    ] {
        assert!(Pipeline::compile(bad.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn an_empty_value_is_written_unless_ignore_empty_value_is_set() {
    let set = r#"{"set":{"field":"a","value":""}}"#;
    assert_eq!(one(set, "{}").unwrap(), r#"{"a":""}"#);
    let null = r#"{"set":{"field":"a","value":null}}"#;
    assert_eq!(one(null, "{}").unwrap(), r#"{"a":null}"#);
}

#[test]
fn a_boolean_converts_both_ways_and_a_float_reports_its_type() {
    let to_bool = r#"{"convert":{"field":"a","type":"boolean"}}"#;
    assert_eq!(one(to_bool, r#"{"a":"false"}"#).unwrap(), r#"{"a":false}"#);
    assert_eq!(
        one(r#"{"trim":{"field":"a"}}"#, r#"{"a":1.5}"#).unwrap_err(),
        "field [a] of type [java.lang.Double] cannot be cast to [java.lang.String]"
    );
}

#[test]
fn field_names_must_not_be_empty_and_templates_are_refused_in_arrays() {
    for bad in [
        r#"{"processors":[{"remove":{"field":""}}]}"#,
        r#"{"processors":[{"remove":{"field":[""]}}]}"#,
        r#"{"processors":[{"set":{"field":"a","value":["{{x}}"]}}]}"#,
    ] {
        assert!(Pipeline::compile(bad.as_bytes()).is_err(), "{bad}");
    }
}
