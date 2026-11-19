//! docs/specs/coordinator-mapping-update: a mapping update is planned the way
//! the shell merges it.

use oss_coordinator::indices::mapping_update::{ATTEMPTS, MappingPlan, plan, status_of};

const A: &str = r#"{"properties":{"a":{"type":"keyword"}}}"#;

fn refused(current: Option<&str>, body: &str) -> (u16, &'static str, String) {
    let r = plan(current, Some(body.as_bytes())).expect_err("refused");
    (r.status, r.kind, r.reason)
}

#[test]
fn a_body_is_required() {
    for body in [None, Some(&b""[..])] {
        let r = plan(Some(A), body).unwrap_err();
        assert_eq!(
            (r.status, r.kind, r.reason.as_str()),
            (400, "missing_body", "a mapping body is required")
        );
    }
}

#[test]
fn a_body_is_merged_over_the_stored_mapping() {
    let merged = r#"{"properties":{"a":{"type":"keyword"},"b":{"type":"long"}}}"#;
    for body in [
        r#"{"properties":{"b":{"type":"long"}}}"#,
        r#"{"_doc":{"properties":{"b":{"type":"long"}}}}"#,
    ] {
        assert_eq!(
            plan(Some(A), Some(body.as_bytes())),
            Ok(MappingPlan::Replace(merged.to_owned())),
            "{body}"
        );
    }
}

#[test]
fn a_change_that_changes_nothing_is_unchanged() {
    assert_eq!(
        plan(Some(A), Some(A.as_bytes())),
        Ok(MappingPlan::Unchanged)
    );
    assert_eq!(
        plan(Some(A), Some(br#"{"properties":{}}"#)),
        Ok(MappingPlan::Unchanged)
    );
    // With nothing stored, the first mapping is always written.
    assert_eq!(
        plan(None, Some(A.as_bytes())),
        Ok(MappingPlan::Replace(A.to_owned()))
    );
    assert_eq!(
        plan(None, Some(br#"{"properties":{}}"#)),
        Ok(MappingPlan::Replace("{}".to_owned()))
    );
}

#[test]
fn refusals_keep_their_type_and_message_with_their_status() {
    assert_eq!(
        refused(Some(A), r#"{"properties":{"a":{"type":"long"}}}"#),
        (
            400,
            "illegal_argument_exception",
            "mapper [a] cannot be changed from type [keyword] to [long]".to_owned()
        )
    );
    let (status, kind, reason) = refused(Some(A), "{");
    assert_eq!((status, kind), (400, "mapper_parsing_exception"));
    assert!(
        reason.starts_with("Failed to parse mapping [_doc]: "),
        "{reason}"
    );
    // A stored mapping that does not parse is refused the same way.
    let (status, kind, _) = refused(Some("{"), A);
    assert_eq!((status, kind), (400, "mapper_parsing_exception"));
    assert_eq!(status_of("mapper_exception"), 500);
    assert_eq!(status_of("unsupported_mapping"), 501);
    assert_eq!(status_of("illegal_argument_exception"), 400);
    assert_eq!(status_of("mapper_parsing_exception"), 400);
}

#[test]
fn the_swap_is_tried_four_times() {
    assert_eq!(ATTEMPTS, 4);
}
