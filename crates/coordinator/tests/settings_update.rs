//! docs/specs/coordinator-settings-update: what a settings update may change.

// ARITH: test counts are small constants.
#![allow(clippy::arithmetic_side_effects)]

use oss_coordinator::indices::settings_update::{ATTEMPTS, Kind, Scope, merge, parse};

struct Fake;

impl Scope for Fake {
    fn classify(&self, key: &str, value: Option<&str>) -> Kind {
        match key {
            "index.serverless.bucket_policy" => match value {
                Some("ok") => Kind::Ours(Ok(())),
                _ => Kind::Ours(Err("bucket policy must be ok".to_owned())),
            },
            "index.refresh_interval"
            | "index.number_of_replicas"
            | "index.search.default_pipeline" => Kind::Core { dynamic: true },
            "index.codec" | "index.number_of_shards" | "index.analysis.x" => {
                Kind::Core { dynamic: false }
            }
            _ => Kind::Unknown,
        }
    }
}

fn refused(body: &str) -> (u16, &'static str, String) {
    let r = parse(Some(body.as_bytes()), &Fake).expect_err("refused");
    (r.status, r.kind, r.reason)
}

fn pairs(v: &[(String, Option<String>)]) -> Vec<(&str, Option<&str>)> {
    v.iter().map(|(k, x)| (k.as_str(), x.as_deref())).collect()
}

#[test]
fn a_body_must_exist_be_an_object_and_name_a_setting() {
    for body in [None, Some(&b""[..]), Some(&b" \n\t"[..])] {
        let r = parse(body, &Fake).unwrap_err();
        assert_eq!(
            (r.status, r.kind, r.reason.as_str()),
            (400, "missing_body", "a settings body is required")
        );
    }
    for body in ["{", "[1]", "3"] {
        let (status, kind, reason) = refused(body);
        assert_eq!((status, kind), (400, "malformed_body"), "{body}");
        assert!(
            reason.starts_with("could not parse the settings body: "),
            "{reason}"
        );
    }
    for body in ["{}", r#"{"settings": {}}"#, r#"{"index": {}}"#] {
        assert_eq!(
            refused(body),
            (400, "missing_body", "no settings were given".to_owned()),
            "{body}"
        );
    }
}

#[test]
fn both_body_shapes_are_read() {
    let want = vec![("index.refresh_interval", Some("5s"))];
    for body in [
        r#"{"settings": {"refresh_interval": "5s"}}"#,
        r#"{"refresh_interval": "5s"}"#,
        r#"{"index": {"refresh_interval": "5s"}}"#,
        r#"{"index.refresh_interval": "5s"}"#,
    ] {
        let got = parse(Some(body.as_bytes()), &Fake).unwrap();
        assert_eq!(pairs(&got), want, "{body}");
    }
    // `settings` that is not an object is not the envelope: the body is the settings, and it is unknown.
    assert_eq!(
        refused(r#"{"settings": 5}"#).2,
        "unknown index setting [index.settings]"
    );
}

#[test]
fn ours_are_checked_and_unknown_are_listed() {
    assert!(parse(Some(br#"{"serverless.bucket_policy": "x"}"#), &Fake).is_err());
    assert_eq!(
        refused(r#"{"index.serverless.bucket_policy": "no"}"#),
        (
            400,
            "illegal_argument_exception",
            "bucket policy must be ok".to_owned()
        )
    );
    let ok = parse(Some(br#"{"index.serverless.bucket_policy": "ok"}"#), &Fake).unwrap();
    assert_eq!(
        pairs(&ok),
        vec![("index.serverless.bucket_policy", Some("ok"))]
    );
    assert_eq!(
        refused(r#"{"zeta": 1}"#),
        (
            400,
            "illegal_argument_exception",
            "unknown index setting [index.zeta]".to_owned()
        )
    );
    assert_eq!(
        refused(r#"{"zeta": 1, "alpha": 2, "refresh_interval": "1s"}"#),
        (
            400,
            "illegal_argument_exception",
            "unknown index settings [index.alpha, index.zeta]".to_owned()
        )
    );
}

#[test]
fn refusals_come_in_the_shells_order_and_words() {
    let mode = "index.serverless.shard_mode is chosen when an index is created and cannot be changed: it decides how the index's units start, and the units an index already has stay as they are";
    let shards = "number_of_shards cannot be changed after an index is created: the shard list lives in the index descriptor and routing is a function of it, so changing the count would re-route every document that already exists. An index that needs a different shard count is a new index";
    let replicas = "number_of_replicas is not supported: a shard has one writer and its durability comes from the object store, not from replica copies. Readers are added by scaling search nodes, not by setting a replica count";
    let pipeline = "index.search.default_pipeline is not supported: an index-level search pipeline is applied by the search path, and this shell's does not read it, so the value would be stored and never run. Name the pipeline on the request instead (?search_pipeline=)";
    let tail = "cannot be changed on a live index. Classic OpenSearch changes these only while an index is closed, and this design has no closed state: an index is either being served or it does not exist. Set them at creation";
    let r = |reason: &str| (501, "unsupported_setting", reason.to_owned());

    assert_eq!(
        refused(r#"{"index.serverless.shard_mode": "fixed"}"#),
        r(mode)
    );
    assert_eq!(refused(r#"{"number_of_shards": 3}"#), r(shards));
    assert_eq!(refused(r#"{"number_of_replicas": 1}"#), r(replicas));
    assert_eq!(
        refused(r#"{"index.search.default_pipeline": "p"}"#),
        r(pipeline)
    );
    assert_eq!(
        refused(r#"{"codec": "x"}"#),
        r(&format!("static index setting [index.codec] {tail}"))
    );
    assert_eq!(
        refused(r#"{"codec": "x", "analysis": {"x": 1}}"#),
        r(&format!(
            "static index settings [index.analysis.x, index.codec] {tail}"
        ))
    );
    // Order: shard mode, shards, replicas, pipeline, statics.
    assert_eq!(
        refused(
            r#"{"index.serverless.shard_mode": "x", "number_of_shards": 3, "number_of_replicas": 1, "index.search.default_pipeline": "p", "codec": "c"}"#
        ),
        r(mode)
    );
    assert_eq!(
        refused(
            r#"{"number_of_shards": 3, "number_of_replicas": 1, "index.search.default_pipeline": "p", "codec": "c"}"#
        ),
        r(shards)
    );
    assert_eq!(
        refused(r#"{"number_of_replicas": 1, "index.search.default_pipeline": "p", "codec": "c"}"#),
        r(replicas)
    );
    assert_eq!(
        refused(r#"{"index.search.default_pipeline": "p", "codec": "c"}"#),
        r(pipeline)
    );
    // Null is no value, and zero replicas is accepted (and the replica key is not a static one).
    let ok = parse(
        Some(br#"{"number_of_replicas": 0, "refresh_interval": null, "index.search.default_pipeline": null}"#),
        &Fake,
    )
    .unwrap();
    assert_eq!(
        pairs(&ok),
        vec![
            ("index.number_of_replicas", Some("0")),
            ("index.refresh_interval", None),
            ("index.search.default_pipeline", None),
        ]
    );
    // A static key is refused even when its value is null: only the replica key is skipped.
    assert_eq!(
        refused(r#"{"codec": null}"#),
        r(&format!("static index setting [index.codec] {tail}"))
    );
}

#[test]
fn unknown_is_decided_before_a_refusal() {
    assert_eq!(
        refused(r#"{"number_of_shards": 3, "zeta": 1}"#),
        (
            400,
            "illegal_argument_exception",
            "unknown index setting [index.zeta]".to_owned()
        )
    );
    // shard_mode is not "unknown": it reaches its own refusal.
    assert_eq!(
        refused(r#"{"index.serverless.shard_mode": "x"}"#).1,
        "unsupported_setting"
    );
}

fn s(key: &str, value: &str) -> (String, Option<String>) {
    (key.to_owned(), Some(value.to_owned()))
}

#[test]
fn a_merge_layers_the_request_over_the_current_settings() {
    let current = vec![s("index.a", "1"), s("index.b", "2"), s("index.z", "9")];
    let request = vec![s("index.b", "3"), s("index.c", "4")];
    let got = merge(&current, &request, false).unwrap();
    assert_eq!(
        pairs(&got),
        vec![
            ("index.a", Some("1")),
            ("index.b", Some("3")),
            ("index.c", Some("4")),
            ("index.z", Some("9")),
        ]
    );
    // preserve_existing: b keeps its value, c is taken.
    let got = merge(&current, &request, true).unwrap();
    assert_eq!(
        pairs(&got),
        vec![
            ("index.a", Some("1")),
            ("index.b", Some("2")),
            ("index.c", Some("4")),
            ("index.z", Some("9")),
        ]
    );
    // A null removes; with preserve_existing it removes only what is not held.
    let removal = vec![("index.a".to_owned(), None), s("index.q", "1")];
    let got = merge(&current, &removal, false).unwrap();
    assert_eq!(
        pairs(&got),
        vec![
            ("index.b", Some("2")),
            ("index.q", Some("1")),
            ("index.z", Some("9"))
        ]
    );
    let got = merge(&current, &removal, true).unwrap();
    assert_eq!(
        pairs(&got),
        vec![
            ("index.a", Some("1")),
            ("index.b", Some("2")),
            ("index.q", Some("1")),
            ("index.z", Some("9"))
        ]
    );
    // The replica key never stays, and removing it from nothing changes nothing.
    let got = merge(&current, &[s("index.number_of_replicas", "0")], false);
    assert_eq!(got, None);
    let with_replicas = vec![s("index.a", "1"), s("index.number_of_replicas", "0")];
    let got = merge(&with_replicas, &[s("index.b", "2")], false).unwrap();
    assert_eq!(
        pairs(&got),
        vec![("index.a", Some("1")), ("index.b", Some("2"))]
    );
    // Nothing changed gives none, in every form of nothing.
    assert_eq!(merge(&current, &[s("index.a", "1")], false), None);
    assert_eq!(merge(&current, &[s("index.a", "7")], true), None);
    assert_eq!(
        merge(&current, &[("index.nope".to_owned(), None)], false),
        None
    );
    assert_eq!(merge(&current, &[], false), None);
}

#[test]
fn the_swap_is_tried_four_times() {
    assert_eq!(ATTEMPTS, 4);
}
