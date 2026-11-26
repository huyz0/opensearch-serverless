//! docs/specs/restore-index-settings AC-1 to AC-3: what a restore's
//! `index_settings` and `ignore_index_settings` read as, refuse and do to
//! the settings of a created index.

use oss_coordinator::snapshot_restore_admin::request;
use oss_coordinator::snapshot_restore_settings::SettingsChange;

fn asked(body: &str) -> Result<SettingsChange, (u16, &'static str, String)> {
    request("backups", "daily", Some(body.as_bytes()))
        .map(|r| r.settings)
        .map_err(|r| (r.status, r.kind, r.reason))
}

fn set(pairs: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.map(str::to_owned)))
        .collect()
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn a_request_without_either_key_changes_nothing() {
    let change = asked(r#"{"indices":"logs"}"#).unwrap();
    assert!(change.is_empty());
    assert_eq!(change, SettingsChange::default());
}

#[test]
fn index_settings_read_nested_flat_and_unprefixed_alike() {
    let want = set(&[
        ("index.refresh_interval", Some("30s")),
        ("index.routing_partition_size", Some("1")),
    ]);
    for body in [
        r#"{"index_settings":{"index":{"refresh_interval":"30s","routing_partition_size":1}}}"#,
        r#"{"index_settings":{"index.refresh_interval":"30s","index.routing_partition_size":1}}"#,
        r#"{"index_settings":{"refresh_interval":"30s","routing_partition_size":"1"}}"#,
    ] {
        let change = asked(body).unwrap();
        assert_eq!(change.set, want, "{body}");
        assert!(change.ignore.is_empty());
    }
    let nulled = asked(r#"{"index_settings":{"index.refresh_interval":null}}"#).unwrap();
    assert_eq!(nulled.set, set(&[("index.refresh_interval", None)]));
}

#[test]
fn ignore_index_settings_read_a_list_or_an_array() {
    let want = names(&["index.refresh_interval", "index.blocks.*"]);
    for body in [
        r#"{"ignore_index_settings":"refresh_interval, index.blocks.*,,"}"#,
        r#"{"ignore_index_settings":["index.refresh_interval","blocks.*"]}"#,
    ] {
        assert_eq!(asked(body).unwrap().ignore, want, "{body}");
    }
}

#[test]
fn a_field_of_the_wrong_shape_is_400() {
    for body in [
        r#"{"index_settings":"a"}"#,
        r#"{"index_settings":[1]}"#,
        r#"{"ignore_index_settings":3}"#,
        r#"{"ignore_index_settings":[1]}"#,
        r#"{"ignore_index_settings":{"a":1}}"#,
    ] {
        let (status, kind, _) = asked(body).unwrap_err();
        assert_eq!(
            (status, kind),
            (400, "illegal_argument_exception"),
            "{body}"
        );
    }
}

#[test]
fn the_settings_core_fixes_cannot_be_modified_or_removed() {
    for key in [
        "index.number_of_shards",
        "index.version.created",
        "index.uuid",
        "index.creation_date",
        "index.history.uuid",
    ] {
        let modify = format!(r#"{{"index_settings":{{"{key}":"1"}}}}"#);
        let (status, kind, why) = asked(&modify).unwrap_err();
        assert_eq!((status, kind), (400, "snapshot_restore_exception"));
        assert_eq!(why, format!("cannot modify setting [{key}] on restore"));
        let clear = format!(r#"{{"index_settings":{{"{key}":null}}}}"#);
        assert_eq!(
            asked(&clear).unwrap_err().2,
            format!("cannot remove setting [{key}] on restore")
        );
        let ignore = format!(r#"{{"ignore_index_settings":["{key}"]}}"#);
        assert_eq!(
            asked(&ignore).unwrap_err().2,
            format!("cannot remove setting [{key}] on restore")
        );
    }
}

fn merged(change: &SettingsChange, captured: &str) -> String {
    String::from_utf8(change.merge(Some(captured.as_bytes())).unwrap()).unwrap()
}

#[test]
fn ignored_settings_go_first_and_given_ones_win() {
    let change = asked(
        r#"{"ignore_index_settings":["index.refresh_interval","index.blocks.*"],
            "index_settings":{"index.refresh_interval":"1s","index.max_result_window":"500"}}"#,
    )
    .unwrap();
    let captured = r#"{"index.refresh_interval":"5s","index.blocks.write":"true","index.blocks.read":"false","index.number_of_shards":"2","index.codec":"zstd"}"#;
    assert_eq!(
        merged(&change, captured),
        r#"{"index.codec":"zstd","index.max_result_window":"500","index.number_of_shards":"2","index.refresh_interval":"1s"}"#
    );
}

#[test]
fn a_wildcard_leaves_the_settings_core_fixes() {
    let change = asked(r#"{"ignore_index_settings":"index.*"}"#).unwrap();
    let captured =
        r#"{"index.number_of_shards":"2","index.uuid":"u","index.refresh_interval":"5s"}"#;
    assert_eq!(
        merged(&change, captured),
        r#"{"index.number_of_shards":"2","index.uuid":"u"}"#
    );
}

#[test]
fn a_null_removes_the_captured_value_and_no_captured_settings_is_an_empty_object() {
    let change = asked(r#"{"index_settings":{"index.refresh_interval":null}}"#).unwrap();
    assert_eq!(merged(&change, r#"{"index.refresh_interval":"5s"}"#), "{}");
    let given = asked(r#"{"index_settings":{"refresh_interval":"2s"}}"#).unwrap();
    assert_eq!(
        String::from_utf8(given.merge(None).unwrap()).unwrap(),
        r#"{"index.refresh_interval":"2s"}"#
    );
}

#[test]
fn captured_keys_without_the_prefix_are_named_as_core_names_them() {
    let change = asked(r#"{"ignore_index_settings":"refresh_interval"}"#).unwrap();
    assert_eq!(
        merged(&change, r#"{"refresh_interval":"5s","index.codec":"zstd"}"#),
        r#"{"index.codec":"zstd"}"#
    );
}

#[test]
fn settings_that_are_not_flat_text_are_501_and_a_quoted_value_is_escaped() {
    let change = asked(r#"{"index_settings":{"index.codec":"zstd"}}"#).unwrap();
    for captured in [r#"{"index":{"codec":"x"}}"#, r#"{"a":1}"#, "[1]", "{"] {
        let refusal = change.merge(Some(captured.as_bytes())).unwrap_err();
        assert_eq!(
            (refusal.status, refusal.kind),
            (501, "unsupported_restore"),
            "{captured}"
        );
    }
    let quoted = asked(r#"{"index_settings":{"index.analysis":"a\"b"}}"#).unwrap();
    assert_eq!(merged(&quoted, "{}"), r#"{"index.analysis":"a\"b"}"#);
}

#[test]
fn what_else_the_shell_refuses_is_still_refused() {
    for key in ["include_global_state"] {
        let body = format!(r#"{{"{key}":true,"index_settings":{{"index.codec":"x"}}}}"#);
        let (status, kind, _) = asked(&body).unwrap_err();
        assert_eq!((status, kind), (501, "unsupported_restore"), "{key}");
    }
}

#[test]
fn a_wildcard_that_is_not_trailing_is_refused_and_a_value_is_trimmed() {
    let (status, kind, _) = asked(r#"{"ignore_index_settings":["index.*.foo"]}"#).unwrap_err();
    assert_eq!((status, kind), (400, "illegal_argument_exception"));
    let change = asked(r#"{"index_settings":{"index.refresh_interval":" 7s "}}"#).unwrap();
    assert_eq!(change.set, set(&[("index.refresh_interval", Some("7s"))]));
}
