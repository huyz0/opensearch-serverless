//! Cluster settings read and written as the Java's `ClusterConfig` and
//! core `Settings` read them (docs/specs/cluster-config/SPEC.md, AC-1 and
//! AC-2).

mod common;

use common::settings_fixture;
use oss_format::{
    CLUSTER_CONFIG_KEY, ClusterSettings, FormatError, MAX_CLUSTER_SETTINGS_BYTES, Setting,
    SettingsRefused, cluster_config_key,
};

fn written(settings: &ClusterSettings) -> String {
    let mut buf = vec![0u8; 4096];
    let n = settings.write(&mut buf).unwrap();
    buf.truncate(n);
    String::from_utf8(buf).unwrap()
}

fn value(text: &str) -> Setting {
    Setting::Value(text.to_owned())
}

fn list(items: &[&str]) -> Setting {
    Setting::List(items.iter().map(|s| (*s).to_owned()).collect())
}

/// Settings read from a flat value, the only way to hold a list.
fn read(flat: &str) -> ClusterSettings {
    ClusterSettings::parse(flat.as_bytes()).unwrap()
}

fn pairs(s: &ClusterSettings) -> Vec<(&str, &Setting)> {
    s.settings().iter().map(|(k, v)| (k.as_str(), v)).collect()
}

/// The changes of one line of `updates.txt`.
fn changes(line: &str) -> Vec<(&str, Option<&str>)> {
    line.split(' ')
        .map(|word| match word.split_once('=') {
            Some((key, value)) => (key, Some(value)),
            None => (word, None),
        })
        .collect()
}

#[test]
fn every_case_reads_as_the_java_reads_it() {
    let cases = settings_fixture("cases.txt");
    let parsed = settings_fixture("parsed.txt");
    assert_eq!(cases.len(), parsed.len());
    assert!(cases.len() >= 30);
    for (case, java) in cases.iter().zip(&parsed) {
        let rust = ClusterSettings::parse(case.as_bytes());
        if java == "refused" {
            assert!(
                matches!(rust, Err(FormatError::Corrupt { .. })),
                "{case}: {rust:?}"
            );
            continue;
        }
        let rust = rust.unwrap_or_else(|e| panic!("{case}: {e}"));
        // The Java keeps a null key and writes it; the Rust reads it as
        // absent, as the Java's `get` answers it.
        let java = java.replace(",\"z\":null", "");
        assert_eq!(written(&rust), java, "{case}");
        // What the Rust writes reads back the same.
        assert_eq!(ClusterSettings::parse(java.as_bytes()).unwrap(), rust);
    }
}

#[test]
fn the_javas_nested_writes_read_as_its_flat_form() {
    let updated = settings_fixture("updated.txt");
    assert_eq!(updated.len(), 10);
    for pair in updated.chunks(2) {
        let read = ClusterSettings::parse(pair[0].as_bytes()).unwrap();
        assert_eq!(written(&read), pair[1]);
    }
}

#[test]
fn updates_merge_as_the_javas() {
    let steps = settings_fixture("updates.txt");
    let updated = settings_fixture("updated.txt");
    assert_eq!(updated.len(), steps.len() * 2);
    let mut current = ClusterSettings::default();
    for (line, java) in steps.iter().zip(updated.chunks(2)) {
        current.apply(&changes(line)).unwrap();
        assert_eq!(written(&current), java[1], "{line}");
    }
}

#[test]
fn an_update_that_makes_a_list_is_refused() {
    let mut s = read(r#"{"y":"1"}"#);
    for asked in [
        &[("x.0", Some("a")), ("x.1", Some("b"))][..],
        &[("x.0", Some("a"))],
        &[("y", None), ("x.0", Some("a"))],
    ] {
        let refused = s.apply(asked).unwrap_err();
        assert!(
            matches!(&refused, SettingsRefused::List { key } if key == "x"),
            "{asked:?}: {refused:?}"
        );
        assert_eq!(s, read(r#"{"y":"1"}"#));
    }
    assert_eq!(
        s.apply(&[("x.0", Some("a"))]).unwrap_err().to_string(),
        "'x' is a list; list-valued settings are not supported on this register"
    );
    // A key that only looks like a list entry is a scalar.
    s.apply(&[("x.1", Some("b")), ("x.10", Some("c"))]).unwrap();
    assert_eq!(s.get("x.1"), Some(&value("b")));
}

#[test]
fn lists_already_held_survive_an_unrelated_update() {
    let mut s = read(r#"{"x":["a","b"],"y":"1"}"#);
    s.apply(&[("y", Some("2")), ("x.1", Some("c"))]).unwrap();
    assert_eq!(
        pairs(&s),
        [
            ("x", &list(&["a", "b"])),
            ("x.1", &value("c")),
            ("y", &value("2"))
        ]
    );
    s.apply(&[("x", None)]).unwrap();
    assert_eq!(s.get("x"), None);
}

#[test]
fn an_update_the_java_refuses_changes_nothing() {
    let mut s = read(r#"{"x":["p"],"y":"1"}"#);
    let before = s.clone();
    // A setting and its list entry, asked together.
    let refused = s
        .apply(&[("y", Some("2")), ("z", Some("s")), ("z.0", Some("q"))])
        .unwrap_err();
    assert_eq!(
        refused,
        SettingsRefused::Conflict {
            prefix: "z".into(),
            key: "z.0".into()
        }
    );
    assert_eq!(
        refused.to_string(),
        "settings can't contain values for [z] and [z.0]"
    );
    // A null list entry, which the Java holds but cannot read back.
    let refused = s.apply(&[("v.0", None), ("v.1", Some("b"))]).unwrap_err();
    assert!(
        matches!(refused, SettingsRefused::Conflict { .. }),
        "{refused:?}"
    );
    // A list made from `p.0.0` beside the list `p`.
    let refused = s
        .apply(&[("w.0", Some("a")), ("w.0.0", Some("c"))])
        .unwrap_err();
    assert!(
        matches!(&refused, SettingsRefused::Conflict { prefix, .. } if prefix == "w"),
        "{refused:?}"
    );
    assert_eq!(s, before);
}

#[test]
fn settings_past_the_bound_are_refused() {
    assert_eq!(MAX_CLUSTER_SETTINGS_BYTES, 256 * 1024);
    // One setting whose flat size is exactly the bound: its key, its value
    // and 6 bytes of quotes and separators.
    let long = "v".repeat(MAX_CLUSTER_SETTINGS_BYTES - 7);
    let mut s = ClusterSettings::default();
    s.apply(&[("k", Some(&long))]).unwrap();
    let mut buf = vec![0u8; MAX_CLUSTER_SETTINGS_BYTES + 64];
    let n = s.write(&mut buf).unwrap();
    assert_eq!(ClusterSettings::parse(&buf[..n]).unwrap(), s);
    // One byte past it.
    let refused = s.apply(&[("k", Some(&format!("{long}v")))]).unwrap_err();
    assert_eq!(
        refused,
        SettingsRefused::TooLarge {
            actual: MAX_CLUSTER_SETTINGS_BYTES + 1
        }
    );
    assert!(
        refused
            .to_string()
            .starts_with("cluster settings too large")
    );
    let past = format!("{{\"k\":\"{long}v\"}}");
    assert!(matches!(
        ClusterSettings::parse(past.as_bytes()),
        Err(FormatError::TooLarge { actual, .. }) if actual == MAX_CLUSTER_SETTINGS_BYTES as u64 + 1
    ));
    // Nesting repeats a key in the flat form, and is counted so: a short
    // value whose flat form is past the bound is refused while it is read.
    let prefix = "p".repeat(1000);
    let fields: Vec<String> = (0..300).map(|i| format!("\"a{i}\":1")).collect();
    let nested = format!("{{\"{prefix}\":{{{}}}}}", fields.join(","));
    assert!(nested.len() < 4096);
    assert!(matches!(
        ClusterSettings::parse(nested.as_bytes()),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn values_round_trip_with_escapes() {
    let s = read(r#"{"a\"b":"line\nbreak\u0001","l":[],"m":["x","é"]}"#);
    assert_eq!(
        pairs(&s),
        [
            ("a\"b", &value("line\nbreak\u{1}")),
            ("l", &list(&[])),
            ("m", &list(&["x", "é"]))
        ]
    );
    let text = written(&s);
    assert_eq!(
        text,
        "{\"a\\\"b\":\"line\\nbreak\\u0001\",\"l\":[],\"m\":[\"x\",\"é\"]}"
    );
    assert_eq!(ClusterSettings::parse(text.as_bytes()).unwrap(), s);
}

#[test]
fn a_short_buffer_says_how_long_it_must_be() {
    let s = read(r#"{"k":"v"}"#);
    let mut buf = [0u8; 4];
    assert_eq!(
        s.write(&mut buf),
        Err(FormatError::TooLarge {
            what: "cluster settings",
            limit: 4,
            actual: 9
        })
    );
}

#[test]
fn malformed_values_are_refused() {
    for bad in [
        &b""[..],
        b"{",
        b"{\"a\":",
        b"{\"a\":[1",
        b"{\"a\":1} x",
        b"not json",
        b"{\"a\":\"\xff\"}",
    ] {
        assert!(ClusterSettings::parse(bad).is_err(), "{bad:?}");
    }
    // The Java refuses a field repeated in one object; this keeps the last.
    let s = ClusterSettings::parse(br#"{"a":"1","a":"2"}"#).unwrap();
    assert_eq!(pairs(&s), [("a", &value("2"))]);
}

#[test]
fn the_key_is_the_javas() {
    assert_eq!(CLUSTER_CONFIG_KEY, "cluster/config/settings");
    assert_eq!(cluster_config_key("").unwrap(), "cluster/config/settings");
    assert_eq!(
        cluster_config_key("dep/").unwrap(),
        "dep/cluster/config/settings"
    );
    assert!(cluster_config_key("dep").is_err());
}
