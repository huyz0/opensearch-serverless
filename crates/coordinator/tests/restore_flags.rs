//! docs/specs/restore-default-flags: the flags a restore accepts at the
//! value this node always applies, and the other value refused.

use oss_coordinator::snapshot_restore_admin::request;

fn kind(body: &str) -> Option<(u16, &'static str)> {
    request("backups", "daily", Some(body.as_bytes()))
        .err()
        .map(|r| (r.status, r.kind))
}

#[test]
fn a_flag_at_the_value_this_node_applies_is_accepted() {
    for key in [
        "partial",
        "include_global_state",
        "include_aliases",
        "ignore_unavailable",
    ] {
        assert_eq!(kind(&format!(r#"{{"{key}":false}}"#)), None, "{key}");
    }
    assert_eq!(
        kind(
            r#"{"partial":false,"include_global_state":false,"include_aliases":false,"ignore_unavailable":false}"#
        ),
        None
    );
}

#[test]
fn a_flag_that_is_not_a_boolean_is_400() {
    for body in [r#"{"partial":"false"}"#, r#"{"include_aliases":0}"#] {
        assert_eq!(
            kind(body),
            Some((400, "illegal_argument_exception")),
            "{body}"
        );
    }
}

#[test]
fn a_refused_key_without_a_default_stays_refused() {
    for key in [
        "alias_write_index_policy",
        "settings",
        "storage_type",
        "allow_no_indices",
        "expand_wildcards",
    ] {
        for value in ["false", "true"] {
            assert_eq!(
                kind(&format!(r#"{{"{key}":{value}}}"#)),
                Some((501, "unsupported_restore")),
                "{key} {value}"
            );
        }
    }
    let r = request("backups", "daily", Some(br#"{"partial":true}"#)).unwrap_err();
    assert!(r.reason.starts_with("partial is not supported: "));
}
