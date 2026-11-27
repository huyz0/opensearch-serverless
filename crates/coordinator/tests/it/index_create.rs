//! docs/specs/coordinator-index-create: a create-index request is read the
//! way the shell reads it.

// ARITH: test counts are small constants.
#![allow(clippy::arithmetic_side_effects)]

use oss_coordinator::indices::create::{CreateRequest, MAX_SHARDS, parse};
use oss_coordinator::route::resolve::Refusal;

fn ok(query: i32, body: &str) -> CreateRequest {
    parse(query, Some(body.as_bytes())).expect("accepted")
}

fn refused(query: i32, body: &str) -> (u16, &'static str, String) {
    let r: Refusal = parse(query, Some(body.as_bytes())).expect_err("refused");
    (r.status, r.kind, r.reason)
}

fn kv(r: &CreateRequest) -> Vec<(&str, Option<&str>)> {
    r.settings
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_deref()))
        .collect()
}

#[test]
fn no_body_is_the_query_shard_count() {
    for body in [None, Some(&b""[..])] {
        let r = parse(1, body).unwrap();
        assert_eq!((r.shards, r.explicit_shards), (1, false));
        assert!(r.mapping.is_none() && r.settings.is_empty() && r.aliases.is_empty());
    }
    let r = parse(3, None).unwrap();
    assert_eq!((r.shards, r.explicit_shards), (3, true));
    let refusal = parse(0, None).unwrap_err();
    assert_eq!((refusal.status, refusal.kind), (400, "invalid_shard_count"));
}

#[test]
fn a_body_that_is_not_an_object_is_malformed() {
    for body in ["{", "[1]", "3", r#"{"a":1,"a":2}"#, "nonsense"] {
        assert_eq!(
            refused(1, body),
            (
                400,
                "malformed_body",
                "the body is not a JSON object".to_owned()
            ),
            "{body}"
        );
    }
}

#[test]
fn a_body_without_an_envelope_is_the_mapping() {
    let body = r#"{ "properties": { "a": { "type": "keyword" } } }"#;
    let r = ok(2, body);
    assert_eq!(r.mapping.as_deref(), Some(body));
    assert!(r.settings.is_empty() && r.aliases.is_empty());
    assert_eq!((r.shards, r.explicit_shards), (2, true));
    let r = ok(1, "{}");
    assert_eq!(
        (r.mapping.as_deref(), r.explicit_shards),
        (Some("{}"), false)
    );
}

#[test]
fn envelope_parts_must_be_objects() {
    for name in ["settings", "mappings", "aliases"] {
        for value in ["null", "1", "[]", r#""x""#] {
            assert_eq!(
                refused(1, &format!(r#"{{"{name}": {value}}}"#)),
                (400, "malformed_body", format!("'{name}' must be an object")),
                "{name} {value}"
            );
        }
    }
    let r = ok(
        1,
        r#"{"mappings": {"properties": {"n": {"type": "long"}}}}"#,
    );
    assert_eq!(
        r.mapping.as_deref(),
        Some(r#"{"properties":{"n":{"type":"long"}}}"#)
    );
}

#[test]
fn settings_are_flattened_prefixed_and_sorted() {
    let r = ok(
        1,
        r#"{"settings": {
            "refresh_interval": "5s",
            "index": {"analysis": {"filter": {"x": {"type": "stop"}}}},
            "index.max_result_window": 500,
            "archived.old": true,
            "foo*": false,
            "tags": ["a", 2],
            "gone": null
        }}"#,
    );
    assert_eq!(
        kv(&r),
        vec![
            ("archived.old", Some("true")),
            ("foo*", Some("false")),
            ("index.analysis.filter.x.type", Some("stop")),
            ("index.gone", None),
            ("index.max_result_window", Some("500")),
            ("index.refresh_interval", Some("5s")),
            ("index.tags", Some(r#"["a",2]"#)),
        ]
    );
    // A dotted key and a nested object that name one key: the later one wins.
    let r = ok(1, r#"{"settings": {"a.b": "1", "a": {"b": "2"}}}"#);
    assert_eq!(kv(&r), vec![("index.a.b", Some("2"))]);
}

#[test]
fn replicas_other_than_zero_are_refused() {
    let words = "number_of_replicas is not supported: a shard has one writer and its durability comes from \
the object store, not from replica copies. Readers are added by scaling search nodes, \
not by setting a replica count.";
    for body in [
        r#"{"settings": {"number_of_replicas": 1}}"#,
        r#"{"settings": {"index.number_of_replicas": "2"}}"#,
        r#"{"settings": {"index": {"number_of_replicas": 1}}}"#,
    ] {
        assert_eq!(
            refused(1, body),
            (501, "unsupported_setting", words.to_owned()),
            "{body}"
        );
    }
    for body in [
        r#"{"settings": {"number_of_replicas": 0}}"#,
        r#"{"settings": {"index": {"number_of_replicas": "0"}}}"#,
        r#"{"settings": {"number_of_replicas": null}}"#,
    ] {
        assert!(ok(1, body).settings.is_empty(), "{body}");
    }
}

#[test]
fn the_shard_count_comes_from_settings_or_the_query_within_bounds() {
    assert_eq!(MAX_SHARDS, 4096);
    let r = ok(1, r#"{"settings": {"number_of_shards": 5}}"#);
    assert_eq!((r.shards, r.explicit_shards), (5, true));
    let r = ok(7, r#"{"settings": {"number_of_shards": "1"}}"#);
    assert_eq!((r.shards, r.explicit_shards), (1, true));
    let r = ok(4, r#"{"settings": {"refresh_interval": "1s"}}"#);
    assert_eq!((r.shards, r.explicit_shards), (4, true));
    let r = ok(1, r#"{"settings": {"index.number_of_shards": 4096}}"#);
    assert_eq!(r.shards, 4096);
    assert_eq!(
        refused(1, r#"{"settings": {"number_of_shards": "many"}}"#),
        (
            400,
            "malformed_body",
            "number_of_shards must be a number".to_owned()
        )
    );
    for (query, body, got) in [
        (1, r#"{"settings": {"number_of_shards": 0}}"#, "0"),
        (1, r#"{"settings": {"number_of_shards": 4097}}"#, "4097"),
        (1, r#"{"settings": {"number_of_shards": -2}}"#, "-2"),
        (5000, r#"{"settings": {}}"#, "5000"),
        (-1, r#"{"mappings": {}}"#, "-1"),
    ] {
        assert_eq!(
            refused(query, body),
            (
                400,
                "invalid_shard_count",
                format!("number_of_shards must be between 1 and 4096, got {got}")
            ),
            "{body}"
        );
    }
}

#[test]
fn shard_and_replica_counts_are_consumed() {
    let r = ok(
        1,
        r#"{"settings": {"number_of_shards": 2, "number_of_replicas": 0, "refresh_interval": "1s"}}"#,
    );
    assert_eq!(kv(&r), vec![("index.refresh_interval", Some("1s"))]);
    let r = ok(
        1,
        r#"{"settings": {"number_of_shards": 2, "number_of_replicas": 0}}"#,
    );
    assert!(r.settings.is_empty());
}

#[test]
fn alias_options_are_kept_and_unsupported_ones_refused() {
    let r = ok(
        1,
        r#"{"aliases": {"a": {"is_write_index": true, "is_hidden": false}, "b": {}, "c": 1, "d": {"is_hidden": "true"}}}"#,
    );
    let names: Vec<&str> = r.aliases.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["a", "b", "c", "d"]);
    let a: Vec<&str> = r.aliases[0].1.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(a, ["is_write_index", "is_hidden"]);
    assert!(r.aliases[1].1.is_empty() && r.aliases[2].1.is_empty());
    assert_eq!(r.shards, 1);
    assert!(r.mapping.is_none());

    let cases = [
        (
            r#"{"filter": {"term": {"a": 1}}}"#,
            "filter is not supported: an alias here names indices and does not narrow them",
        ),
        (
            r#"{"routing": "1"}"#,
            "routing on an alias is not supported: a document is placed by its id alone",
        ),
        (
            r#"{"index_routing": "1"}"#,
            "routing on an alias is not supported: a document is placed by its id alone",
        ),
        (
            r#"{"search_routing": "1"}"#,
            "routing on an alias is not supported: a document is placed by its id alone",
        ),
        (
            r#"{"is_hidden": true}"#,
            "is_hidden is not supported: aliases are found by name and never listed, so there is nothing to hide from",
        ),
    ];
    for (options, words) in cases {
        let body = format!(
            r#"{{"aliases": {{"ok": {{}}, "bad": {options}, "later": {{"filter": {{}}}}}}}}"#
        );
        assert_eq!(
            refused(1, &body),
            (501, "unsupported_alias_option", words.to_owned()),
            "{options}"
        );
    }
    // null is as good as absent
    assert!(
        parse(
            1,
            Some(br#"{"aliases": {"a": {"filter": null, "routing": null}}}"#)
        )
        .is_ok()
    );
}
