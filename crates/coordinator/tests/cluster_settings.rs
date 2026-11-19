//! docs/specs/coordinator-cluster-settings: the cluster settings decisions.

// ARITH: test counts are small constants.
#![allow(clippy::arithmetic_side_effects)]

use oss_coordinator::cluster::settings::{Shown, ack_body, parse_update, settings_body};

fn no_plugins(_: &[(String, Option<String>)]) -> Result<(), String> {
    Ok(())
}

fn refused(body: &str) -> (u16, &'static str, String) {
    let r = parse_update(Some(body.as_bytes()), 0, &no_plugins).expect_err("refused");
    (r.status, r.kind, r.reason)
}

fn one(key: &str, value: &str) -> String {
    format!(r#"{{"persistent":{{"{key}":{value}}}}}"#)
}

#[test]
fn a_body_needs_a_persistent_object() {
    let need = (
        400,
        "illegal_argument_exception",
        "a settings update needs a 'persistent' object".to_owned(),
    );
    for body in [None, Some(&b""[..])] {
        let r = parse_update(body, 0, &no_plugins).unwrap_err();
        assert_eq!((r.status, r.kind, r.reason), need);
    }
    for body in [
        "{}",
        r#"{"transient":{}}"#,
        r#"{"persistent":null}"#,
        r#"{"persistent":3}"#,
        r#"{"persistent":[1]}"#,
    ] {
        assert_eq!(refused(body), need, "{body}");
    }
    for body in [
        r#"{"persistent":{"a":1},"transient":null}"#,
        r#"{"persistent":{"a":1},"transient":"x"}"#,
    ] {
        assert_eq!(
            refused(body),
            (
                400,
                "illegal_argument_exception",
                "'transient' settings must be an object".to_owned()
            ),
            "{body}"
        );
    }
    for body in [
        "{",
        "[1]",
        "3",
        " \n",
        r#"{"persistent":{"a":1},"persistent":{"b":2}}"#,
    ] {
        let (status, kind, _) = refused(body);
        assert_eq!((status, kind), (400, "x_content_parse_exception"), "{body}");
    }
}

#[test]
fn nothing_to_update_is_a_validation_failure() {
    for body in [
        r#"{"persistent":{}}"#,
        r#"{"persistent":{"a":{}},"transient":{"b":{"c":{}}}}"#,
    ] {
        assert_eq!(
            refused(body),
            (
                400,
                "action_request_validation_exception",
                "Validation Failed: 1: no settings to update;".to_owned()
            ),
            "{body}"
        );
    }
}

#[test]
fn a_transient_setting_is_refused_first() {
    // The list and the bad bucket would each be refused; the transient comes first.
    let body = r#"{"persistent":{"serverless.bucket":"x","l":[1]},"transient":{"a":1}}"#;
    assert_eq!(
        refused(body),
        (
            501,
            "status_exception",
            "transient settings are not supported; use persistent".to_owned()
        )
    );
}

#[test]
fn persistent_settings_come_back_flat_and_sorted() {
    let body = r#"{"persistent":{"z":1.5,"b":{"y":true,"x":"s","gone":null},"a":10,"e":{}}}"#;
    let got = parse_update(Some(body.as_bytes()), 0, &no_plugins).unwrap();
    let got: Vec<_> = got
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_deref()))
        .collect();
    assert_eq!(
        got,
        [
            ("a", Some("10")),
            ("b.gone", None),
            ("b.x", Some("s")),
            ("b.y", Some("true")),
            ("z", Some("1.5")),
        ]
    );
}

#[test]
fn a_list_is_refused_by_the_first_key() {
    let body = r#"{"persistent":{"z":[1],"b":{"c":["x"]},"n":null}}"#;
    assert_eq!(
        refused(body),
        (
            501,
            "status_exception",
            "'b.c' is a list; list-valued settings are not supported on this register".to_owned()
        )
    );
    assert!(parse_update(Some(br#"{"persistent":{"n":null}}"#), 0, &no_plugins).is_ok());
}

#[test]
fn the_named_settings_are_checked() {
    let cases = [
        (
            "serverless.bucket",
            r#""maybe""#,
            "bucket must be [auto] or [off], got [maybe]",
        ),
        (
            "serverless.segment_placement",
            r#""disk""#,
            "segment placement must be [segment_blob] or [node_pack], got [disk]",
        ),
        (
            "serverless.tail_overlay",
            r#""yes""#,
            "tail overlay must be [true] or [false], got [yes]",
        ),
        (
            "serverless.refresh_visibility",
            r#""""#,
            "refresh visibility must be one of [owner, serving, everywhere], got []",
        ),
        (
            "serverless.refresh_visibility",
            r#""nobody""#,
            "refresh visibility must be one of [owner, serving, everywhere], got [nobody]",
        ),
    ];
    for (key, value, reason) in cases {
        assert_eq!(
            refused(&one(key, value)),
            (400, "status_exception", reason.to_owned()),
            "{key} {value}"
        );
    }
    for (key, value) in [
        ("serverless.bucket", r#""AUTO""#),
        ("serverless.bucket", r#""Off""#),
        ("serverless.bucket", r#""""#),
        ("serverless.bucket", "null"),
        ("serverless.segment_placement", r#""Node_Pack""#),
        ("serverless.segment_placement", r#""""#),
        ("serverless.tail_overlay", "true"),
        ("serverless.tail_overlay", r#""FALSE""#),
        ("serverless.refresh_visibility", r#""Serving""#),
        ("serverless.refresh_visibility", "null"),
    ] {
        assert!(
            parse_update(Some(one(key, value).as_bytes()), 0, &no_plugins).is_ok(),
            "{key} {value}"
        );
    }
}

#[test]
fn the_checkpoint_age_is_bounded_by_the_commit_period() {
    let key = "serverless.wal.node_log.commits.checkpoint_millis";
    let run = |value: &str, period: u64| {
        parse_update(Some(one(key, value).as_bytes()), period, &no_plugins)
    };
    assert!(run("60000", 0).is_ok());
    assert!(run(r#" " 1000 ""#, 0).is_ok());
    assert!(run("150000", 0).is_ok());
    assert!(run("null", 0).is_ok());
    let e = run(r#""soon""#, 0).unwrap_err();
    assert_eq!((e.status, e.kind), (400, "status_exception"));
    assert_eq!(
        e.reason,
        format!("[{key}] must be a number of milliseconds, got [soon]")
    );
    let e = run("999", 0).unwrap_err();
    assert_eq!(
        e.reason,
        format!(
            "[{key}]: the checkpoint age must be between 1000 ms (2 commit periods of 0 ms, and at \
             least 1000 ms) and 150000 ms (half the 300000 ms pack deadline), got 999"
        )
    );
    assert!(run("150001", 0).is_err());
    let e = run("-5", 0).unwrap_err();
    assert!(e.reason.ends_with(", got -5"), "{}", e.reason);
    // Twice a 2,000 ms period is 4,000 ms.
    assert!(run("3999", 2000).is_err());
    assert!(run("4000", 2000).is_ok());
    // A period of 75,001 ms leaves no age: twice it is past the bound.
    let e = run("60000", 75_001).unwrap_err();
    assert_eq!(
        e.reason,
        format!(
            "[{key}]: the commit period of 75001 ms leaves no checkpoint age: it must be at least 2 \
             commit periods and at most 150000 ms (half the 300000 ms pack deadline)"
        )
    );
    assert!(run("150000", 75_000).is_ok());
}

#[test]
fn the_plugin_check_runs_last() {
    use std::cell::RefCell;
    let seen = RefCell::new(Vec::new());
    let plugins = |flat: &[(String, Option<String>)]| {
        seen.borrow_mut().push(flat.len());
        if flat.iter().any(|(k, _)| k == "plugin.bad") {
            Err("plugin setting plugin.bad is not valid".to_owned())
        } else {
            Ok(())
        }
    };
    let r = parse_update(
        Some(br#"{"persistent":{"plugin.bad":1,"x":2}}"#),
        0,
        &plugins,
    )
    .unwrap_err();
    assert_eq!(
        (r.status, r.kind, r.reason.as_str()),
        (
            400,
            "status_exception",
            "plugin setting plugin.bad is not valid"
        )
    );
    assert_eq!(*seen.borrow(), [2]);
    // An earlier refusal means the plugins are not asked.
    let body = br#"{"persistent":{"plugin.bad":1,"serverless.bucket":"x"}}"#;
    assert!(parse_update(Some(body), 0, &plugins).is_err());
    assert_eq!(*seen.borrow(), [2]);
}

fn shown(key: &str, value: &str) -> (String, Shown) {
    (key.to_owned(), Shown::Value(value.to_owned()))
}

#[test]
fn the_bodies_nest_or_stay_flat() {
    let settings = vec![
        shown("a.b.c", "1"),
        shown("a.b.d", "2"),
        shown("z", "t\"q"),
        ("l.m".to_owned(), Shown::List(vec!["x".into(), "y".into()])),
    ];
    assert_eq!(
        settings_body(&settings, false, false),
        r#"{"persistent":{"a":{"b":{"c":"1","d":"2"}},"l":{"m":["x","y"]},"z":"t\"q"},"transient":{}}"#
    );
    assert_eq!(
        settings_body(&settings, true, true),
        r#"{"persistent":{"a.b.c":"1","a.b.d":"2","l.m":["x","y"],"z":"t\"q"},"transient":{},"defaults":{}}"#
    );
    assert_eq!(
        ack_body(&settings, false),
        r#"{"acknowledged":true,"persistent":{"a":{"b":{"c":"1","d":"2"}},"l":{"m":["x","y"]},"z":"t\"q"},"transient":{}}"#
    );
    assert_eq!(
        settings_body(&[], false, false),
        r#"{"persistent":{},"transient":{}}"#
    );
    // A key that is also the start of another is written flat beside it.
    let both = vec![shown("a", "1"), shown("a.b", "2"), shown("a.c.d", "3")];
    assert_eq!(
        settings_body(&both, false, false),
        r#"{"persistent":{"a":"1","a.b":"2","a.c.d":"3"},"transient":{}}"#
    );
    // Nesting stops at a bounded depth: the rest of the name is written flat.
    let deep = vec![shown(&vec!["k"; 80].join("."), "v")];
    let body = settings_body(&deep, false, false);
    let flat_tail = vec!["k"; 16].join(".");
    assert!(body.contains(&format!(r#""{flat_tail}":"v""#)), "{body}");
    assert_eq!(body.matches('{').count(), 64 + 3);
}
