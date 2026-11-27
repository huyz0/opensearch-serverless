//! What the search pipeline admin routes decide (docs/specs/search-pipelines/SPEC.md).

use oss_coordinator::pipeline_admin::validate_search;

#[test]
fn a_supported_definition_is_accepted_and_a_wrong_one_is_400() {
    assert!(
        validate_search(br#"{"request_processors":[{"oversample":{"sample_factor":2}}]}"#).is_ok()
    );
    let r = validate_search(b"[]").expect_err("refused");
    assert_eq!((r.status, r.kind), (400, "invalid_pipeline"));
    let r = validate_search(br#"{"request_processors":[{"nope":{}}]}"#).expect_err("refused");
    assert_eq!((r.status, r.kind), (400, "invalid_pipeline"));
}

#[test]
fn a_processor_this_node_does_not_build_is_501_by_name() {
    let r = validate_search(br#"{"response_processors":[{"collapse":{"field":"a"}}]}"#)
        .expect_err("refused");
    assert_eq!((r.status, r.kind), (501, "not_implemented"));
    assert!(
        r.reason.contains("[collapse]") && r.reason.contains("truncate_hits"),
        "{}",
        r.reason
    );
}
