//! Tests of the `collapse` parser; compiled only under `#[cfg(test)]`.

use super::collapse::Collapse;
use super::parse_chunks;

fn parse(body: &str) -> Result<Option<Collapse>, crate::failure::Failure> {
    let text = format!(r#"{{"collapse":{body}}}"#);
    parse_chunks([text.as_bytes()])
        .map(|s| s.collapse)
        .map_err(super::DslError::into_failure)
}

#[test]
fn a_field_is_read_and_the_group_bound_is_checked() {
    let c = parse(r#"{"field":"tag","max_concurrent_group_searches":4}"#)
        .expect("parses")
        .expect("a collapse");
    assert_eq!(c.field, "tag");
}

#[test]
fn what_stock_refuses_is_refused_as_stock_does() {
    for (body, message) in [
        (r#"{}"#, "Required [field]"),
        (r#"{"field":""}"#, "field name is null or empty"),
        (
            r#"{"field":"a","max_concurrent_group_searches":0}"#,
            "maxConcurrentGroupRequests` must be positive",
        ),
        (r#"{"field":"a","x":1}"#, "[collapse] unknown field [x]"),
        (
            r#"{"field":"a","inner_hits":{"name":"n"}}"#,
            "[collapse] [inner_hits] is not supported by this node yet",
        ),
    ] {
        let reason = parse(body).expect_err("fails").reason;
        assert!(reason.ends_with(message), "{body}: {reason}");
    }
}
