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
    let body = format!(r#"{{{RENAME},"index_settings":{{"index.refresh_interval":" 7s "}}}}"#);
    let reply = send(&r, "POST", RESTORE, Some(&body));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        settings_of(&r, "copy"),
        r#"{"index.refresh_interval":"7s"}"#
    );
}

#[test]
fn ignore_and_null_remove_a_captured_setting() {
    let r = taken(true);
    let set = format!(
        r#"{{{RENAME},"index_settings":{{"index.refresh_interval":"7s","index.codec":"best_compression"}}}}"#
    );
    assert_eq!(send(&r, "POST", RESTORE, Some(&set)).status, 200);
    // The null leaf removes the given key and ignore removes the other.
    let body = r#"{"rename_pattern":"logs","rename_replacement":"two","index_settings":{"index.refresh_interval":null},"ignore_index_settings":["index.codec"]}"#;
    assert_eq!(send(&r, "POST", RESTORE, Some(body)).status, 200);
    assert_eq!(settings_of(&r, "two"), "{}");
}

#[test]
fn a_wildcard_that_is_not_trailing_is_400() {
    let r = taken(true);
    let body = format!(r#"{{{RENAME},"ignore_index_settings":["index.*.foo"]}}"#);
    assert_eq!(send(&r, "POST", RESTORE, Some(&body)).status, 400);
}

#[test]
fn a_request_with_both_keys_costs_what_one_with_neither_costs() {
    let a = taken(true);
    let b = taken(true);
    let (a0, b0) = (requests(&a), requests(&b));
    let plain = format!("{{{RENAME}}}");
    let empty = format!(
        r#"{{{RENAME},"index_settings":{{"index.refresh_interval":"7s"}},"ignore_index_settings":["index.codec"]}}"#
    );
    assert_eq!(send(&a, "POST", RESTORE, Some(&plain)).status, 200);
    assert_eq!(send(&b, "POST", RESTORE, Some(&empty)).status, 200);
    assert_eq!(requests(&a) - a0, requests(&b) - b0);
    assert_ne!(settings_of(&a, "copy"), settings_of(&b, "copy"));
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

// docs/specs/extension-analyzer AC-7: a restore cannot add a pin through
// `index_settings`; the key is refused and nothing is created.
#[test]
fn a_restore_cannot_add_an_analyzer_pin() {
    let r = taken(true);
    let hash = "0".repeat(64);
    let body = format!(
        r#"{{{RENAME},"index_settings":{{"index.analysis.analyzer.ext_stem.type":"serverless_module","index.analysis.analyzer.ext_stem.module":"{hash}","index.analysis.analyzer.ext_stem.version":"1"}}}}"#
    );
    let reply = send(&r, "POST", RESTORE, Some(&body));
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(
        reply.text().contains("index.analysis.analyzer.ext_stem"),
        "{}",
        reply.text()
    );
    assert!(
        r.descriptors
            .get("copy", &mut vec![0u8; 1 << 16])
            .unwrap()
            .is_none()
    );
}

const PIN: &str = r#"{"index.analysis.analyzer.ext_tok.type":"ext_tok","index.analysis.analyzer.ext_tok.module":"tok","index.analysis.analyzer.ext_tok.version":"3"}"#;

fn pin_the_analyzer(r: &crate::snapshot_restore::Rig) {
    let mut buf = vec![0u8; 1 << 16];
    let generation = r.descriptors.generation_of("logs", &mut buf).unwrap();
    let mut record = oss_format::IndexRecord::new("logs", "u1", 2);
    record.settings = PIN.as_bytes();
    let mut out = vec![0u8; 1 << 16];
    assert!(
        r.descriptors
            .update(&record, generation, &mut out)
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_restore_keeps_the_analyzer_pin_the_snapshot_captured() {
    let r = crate::snapshot_restore::taken_with_aliases(true, false, pin_the_analyzer);
    let reply = send(&r, "POST", RESTORE, Some(&format!("{{{RENAME}}}")));
    assert_eq!(reply.status, 200, "{}", reply.text());
    let restored = settings_of(&r, "copy");
    for key in ["type", "module", "version"] {
        assert!(
            restored.contains(&format!("index.analysis.analyzer.ext_tok.{key}")),
            "{restored}"
        );
    }
    assert!(restored.contains(r#""index.analysis.analyzer.ext_tok.version":"3""#));
}
