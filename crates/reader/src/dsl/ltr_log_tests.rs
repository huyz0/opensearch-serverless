//! Tests of the `ext.ltr_log` parser; compiled only under `#[cfg(test)]`.

use super::ltr_log::LogSpec;
use crate::dsl::parse_chunks;

fn specs(ext: &str) -> Result<Vec<LogSpec>, String> {
    parse_chunks([format!(r#"{{"ext":{ext}}}"#).as_bytes()])
        .map(|s| s.ltr_log)
        .map_err(|e| e.into_failure().reason)
}

#[test]
fn a_spec_or_an_array_of_specs_parses() {
    let one = specs(r#"{"ltr_log":{"log_specs":{"name":"a","named_query":"q"}}}"#).expect("one");
    assert_eq!(
        one,
        vec![LogSpec {
            name: "a".into(),
            named_query: Some("q".into()),
            rescore_index: None,
            missing_as_zero: false,
        }]
    );
    let two = specs(
        r#"{"ltr_log":{"log_specs":[{"name":"a","rescore_index":"1","missing_as_zero":true},{"name":"b","named_query":"q"}]}}"#,
    )
    .expect("two");
    assert_eq!(two.len(), 2);
    assert_eq!(
        (two[0].rescore_index, two[0].missing_as_zero),
        (Some(1), true)
    );
}

#[test]
fn an_empty_ext_and_no_ext_log_nothing() {
    assert_eq!(specs("{}"), Ok(Vec::new()));
    assert_eq!(specs(r#"{"ltr_log":{}}"#), Ok(Vec::new()));
}

#[test]
fn malformed_specs_are_refused_by_name() {
    for (ext, want) in [
        (r#"{"ltr_log":3}"#, "must be an object"),
        (r#"{"ltr_log":{"x":1}}"#, "takes only [log_specs]"),
        (
            r#"{"ltr_log":{"log_specs":3}}"#,
            "must be an object or an array",
        ),
        (r#"{"ltr_log":{"log_specs":[3]}}"#, "holds objects"),
        (
            r#"{"ltr_log":{"log_specs":{"name":"a"}}}"#,
            "one of [named_query] or [rescore_index]",
        ),
        (
            r#"{"ltr_log":{"log_specs":{"name":"a","named_query":"q","rescore_index":0}}}"#,
            "one of [named_query] or [rescore_index]",
        ),
        (
            r#"{"ltr_log":{"log_specs":{"named_query":"q"}}}"#,
            "needs a [name]",
        ),
        (
            r#"{"ltr_log":{"log_specs":{"name":"a","rescore_index":-1}}}"#,
            "must not be negative",
        ),
        (
            r#"{"ltr_log":{"log_specs":{"name":"a","zzz":1}}}"#,
            "unknown field [zzz]",
        ),
        (
            r#"{"rerank":{}}"#,
            "[ext] section [rerank] is not supported",
        ),
    ] {
        let reason = specs(ext).expect_err(ext);
        assert!(reason.contains(want), "{ext}: {reason}");
    }
}

#[test]
fn more_than_the_most_logs_are_refused() {
    let one = r#"{"name":"a","named_query":"q"}"#;
    let many = vec![one; 17].join(",");
    let reason = specs(&format!(r#"{{"ltr_log":{{"log_specs":[{many}]}}}}"#)).expect_err("many");
    assert!(reason.contains("too many log specs"), "{reason}");
}
