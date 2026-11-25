//! docs/specs/restore-index-settings: `index_settings` and
//! `ignore_index_settings` on a restore, applied before anything is written,
//! at the request cost of a restore without them.

use crate::snapshot_restore::{RESTORE, requests, send, taken};

fn settings_of(r: &crate::snapshot_restore::Rig, name: &str) -> String {
    let mut buf = vec![0u8; 1 << 16];
    let found = r
        .descriptors
        .get(name, &mut buf)
        .unwrap()
        .expect("descriptor");
    String::from_utf8(found.settings.to_vec()).unwrap()
}

const RENAME: &str = r#""rename_pattern":"logs","rename_replacement":"copy""#;

#[test]
fn index_settings_are_stored_on_the_restored_index() {
    let r = taken(true);
    let body = format!(r#"{{{RENAME},"index_settings":{{"index.refresh_interval":"7s"}}}}"#);
    let reply = send(&r, "POST", RESTORE, Some(&body));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(settings_of(&r, "copy").contains("7s"));
}

#[test]
fn a_request_with_neither_key_costs_what_one_with_empty_keys_costs() {
    let a = taken(true);
    let b = taken(true);
    let (a0, b0) = (requests(&a), requests(&b));
    let plain = format!("{{{RENAME}}}");
    let empty = format!(r#"{{{RENAME},"index_settings":{{}},"ignore_index_settings":[]}}"#);
    assert_eq!(send(&a, "POST", RESTORE, Some(&plain)).status, 200);
    assert_eq!(send(&b, "POST", RESTORE, Some(&empty)).status, 200);
    assert_eq!(requests(&a) - a0, requests(&b) - b0);
    assert_eq!(settings_of(&a, "copy"), settings_of(&b, "copy"));
}

#[test]
fn a_fixed_setting_is_refused_before_anything_is_written() {
    let r = taken(true);
    let body = format!(r#"{{{RENAME},"index_settings":{{"index.number_of_shards":"5"}}}}"#);
    let reply = send(&r, "POST", RESTORE, Some(&body));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(reply.text().contains("snapshot_restore_exception"));
    let ignore = format!(r#"{{{RENAME},"ignore_index_settings":["index.uuid"]}}"#);
    assert_eq!(send(&r, "POST", RESTORE, Some(&ignore)).status, 400);
    // No index was created.
    assert!(
        r.descriptors
            .get("copy", &mut vec![0u8; 1 << 16])
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_bad_value_is_400_and_an_unknown_key_501() {
    let r = taken(true);
    let bad = format!(r#"{{{RENAME},"index_settings":{{"index.refresh_interval":"soon"}}}}"#);
    assert_eq!(send(&r, "POST", RESTORE, Some(&bad)).status, 400);
    let unknown = format!(r#"{{{RENAME},"index_settings":{{"index.no_such_thing":"1"}}}}"#);
    assert_eq!(send(&r, "POST", RESTORE, Some(&unknown)).status, 501);
    assert!(
        r.descriptors
            .get("copy", &mut vec![0u8; 1 << 16])
            .unwrap()
            .is_none()
    );
}
