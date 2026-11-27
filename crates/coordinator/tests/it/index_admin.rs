//! docs/specs/coordinator-index-admin: the index calls that read and delete.

use oss_coordinator::indices::admin::{
    View, alias_failed_after_create, check_name, create_ack, delete_ack, delete_check,
    delete_failed, no_such_index, view_of,
};
use oss_coordinator::route::resolve::Refusal;

fn parts(r: &Refusal) -> (u16, &str, &str) {
    (r.status, r.kind, r.reason.as_str())
}

#[test]
fn reserved_and_backing_names_are_refused() {
    let reserved = |n: &str| {
        format!(
            "'{n}' is not an index: names beginning with an underscore are reserved for APIs, and this shell does not implement this one. An index name that is not there answers 404; this is a 501 because there is nothing here to find under any name"
        )
    };
    for creating in [false, true] {
        let r = check_name("_cat", creating).unwrap_err();
        assert_eq!(
            parts(&r),
            (501, "not_implemented", reserved("_cat").as_str())
        );
    }
    let r = check_name("_", false).unwrap_err();
    assert_eq!(r.status, 501);

    let backing = check_name(".ds-web-000001", true).unwrap_err();
    assert_eq!(
        parts(&backing),
        (
            400,
            "illegal_argument_exception",
            "index name [.ds-web-000001] must not start with '.ds-': that prefix names a data stream's backing indices, which its rollover creates"
        )
    );
    // Only a create refuses the backing prefix.
    assert!(check_name(".ds-web-000001", false).is_ok());
    // Underscore is decided first, and ordinary names pass.
    assert_eq!(check_name("_x", true).unwrap_err().status, 501);
    for ok in ["logs", "a_b", ".hidden", "ds-x", "x-_y"] {
        assert!(check_name(ok, true).is_ok(), "{ok}");
    }
}

#[test]
fn the_view_follows_the_path_and_the_setting_name() {
    assert_eq!(view_of("/idx/_settings", None), View::Settings);
    assert_eq!(
        view_of("/idx/_settings/refresh_interval", Some("refresh_interval")),
        View::Settings
    );
    assert_eq!(view_of("/idx/_mapping", None), View::Mapping);
    assert_eq!(view_of("/idx/_mappings", None), View::Mapping);
    assert_eq!(view_of("/idx", None), View::Full);
    assert_eq!(view_of("/idx/_mapping", Some("x")), View::Settings);
    assert_eq!(view_of("/_settings_not_last/x", None), View::Full);
}

#[test]
fn a_stream_write_index_cannot_be_deleted() {
    assert!(delete_check("i", None).is_ok());
    let r = delete_check("i", Some("s")).unwrap_err();
    assert_eq!(
        parts(&r),
        (
            400,
            "illegal_argument_exception",
            "index [i] is the write index for data stream [s] and cannot be deleted"
        )
    );
}

#[test]
fn a_failed_delete_is_a_conflict_or_not_found() {
    assert_eq!(
        parts(&delete_failed("i", true)),
        (
            409,
            "version_conflict_engine_exception",
            "index [i] changed concurrently; nothing was deleted, retry"
        )
    );
    assert_eq!(
        parts(&delete_failed("i", false)),
        (404, "index_not_found", "no such index: i")
    );
    assert_eq!(
        parts(&no_such_index("x")),
        (404, "index_not_found", "no such index: x")
    );
}

#[test]
fn acknowledgements_are_the_shells_bodies() {
    assert_eq!(
        create_ack("logs", 3),
        r#"{"acknowledged":true,"shards_acknowledged":true,"index":"logs","shards":3}"#
    );
    assert_eq!(
        delete_ack("logs"),
        r#"{"acknowledged":true,"index":"logs"}"#
    );
    assert_eq!(
        delete_ack("we\"ird\\name"),
        r#"{"acknowledged":true,"index":"we\"ird\\name"}"#
    );
}

#[test]
fn a_failed_alias_after_a_create_says_the_index_exists() {
    let refused = Refusal::new(
        409,
        "illegal_argument_exception",
        "alias [a] is also an index",
    );
    let r = alias_failed_after_create("i", "a", &refused);
    assert_eq!(
        parts(&r),
        (
            409,
            "illegal_argument_exception",
            "index [i] was created, and its alias [a] was not: alias [a] is also an index"
        )
    );
}
