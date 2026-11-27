//! docs/specs/create-inherit/SPEC.md: what a create request carries once the
//! template it inherits from is layered under it.

use oss_coordinator::indices::create::{CreateRequest, parse};
use oss_coordinator::indices::create_inherit::inherit;
use oss_coordinator::templates::{Inherited, resolve};

fn from_template(template: &str) -> Inherited {
    resolve(std::iter::once(("t", template.as_bytes())), |_| None, "x").unwrap()
}

fn request(query: i32, body: Option<&str>) -> CreateRequest {
    parse(query, body.map(str::as_bytes)).unwrap()
}

fn kv(r: &CreateRequest) -> Vec<(&str, Option<&str>)> {
    r.settings
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_deref()))
        .collect()
}

const ALL: &str = r#"{"index_patterns":["*"],"template":{"settings":{"index":{"number_of_shards":"4","number_of_replicas":"2","refresh_interval":"5s"},"codec":"best"},"mappings":{"properties":{"a":{"type":"keyword"}}}}}"#;

#[test]
fn nothing_inherited_changes_nothing() {
    let none = from_template(r#"{"index_patterns":["zzz"]}"#);
    for given in [
        request(1, None),
        request(
            3,
            Some(
                r#"{"settings":{"refresh_interval":"1s"},"mappings":{"properties":{"b":{"type":"text"}}}}"#,
            ),
        ),
    ] {
        assert_eq!(inherit(given.clone(), &none).unwrap(), given);
    }
}

#[test]
fn a_template_supplies_the_shard_count_unless_the_request_names_one() {
    let inherited = from_template(ALL);
    let got = inherit(request(1, None), &inherited).unwrap();
    assert_eq!(got.shards, 4);
    // Named in the body or the query: the request wins, and says so.
    let body = inherit(
        request(1, Some(r#"{"settings":{"number_of_shards":2}}"#)),
        &inherited,
    )
    .unwrap();
    assert_eq!((body.shards, body.explicit_shards), (2, true));
    let query = inherit(request(3, None), &inherited).unwrap();
    assert_eq!((query.shards, query.explicit_shards), (3, true));
}

#[test]
fn a_template_shard_count_the_create_would_refuse_is_refused() {
    for bad in ["0", "4097", "-1"] {
        let template = format!(
            r#"{{"index_patterns":["*"],"template":{{"settings":{{"index":{{"number_of_shards":"{bad}"}}}}}}}}"#
        );
        let err = inherit(request(1, None), &from_template(&template)).unwrap_err();
        assert_eq!(
            (err.status, err.kind),
            (400, "invalid_shard_count"),
            "{bad}"
        );
    }
    // Not a number: as the Java does, the request's own count stands.
    let word =
        r#"{"index_patterns":["*"],"template":{"settings":{"index":{"number_of_shards":"many"}}}}"#;
    assert_eq!(
        inherit(request(1, None), &from_template(word))
            .unwrap()
            .shards,
        1
    );
    assert_eq!(
        inherit(
            request(1, None),
            &from_template(
                r#"{"index_patterns":["*"],"template":{"settings":{"number_of_shards":6}}}"#
            )
        )
        .unwrap()
        .shards,
        6
    );
}

#[test]
fn template_settings_sit_under_the_requests_and_structural_ones_are_dropped() {
    let inherited = from_template(ALL);
    let got = inherit(
        request(
            1,
            Some(r#"{"settings":{"refresh_interval":"1s","index.extra":"y"}}"#),
        ),
        &inherited,
    )
    .unwrap();
    assert_eq!(
        kv(&got),
        [
            ("index.codec", Some("best")),
            ("index.extra", Some("y")),
            ("index.refresh_interval", Some("1s")),
        ]
    );
    let plain = inherit(request(1, None), &inherited).unwrap();
    assert_eq!(
        kv(&plain),
        [
            ("index.codec", Some("best")),
            ("index.refresh_interval", Some("5s"))
        ]
    );
    // A request's null removes what the template set, as a null does.
    let nulled = inherit(
        request(1, Some(r#"{"settings":{"refresh_interval":null}}"#)),
        &inherited,
    )
    .unwrap();
    assert!(kv(&nulled).contains(&("index.refresh_interval", None)));
}

#[test]
fn template_mappings_sit_under_the_requests_and_a_field_replaces() {
    let inherited = from_template(ALL);
    let plain = inherit(request(1, None), &inherited).unwrap();
    assert_eq!(
        plain.mapping.as_deref(),
        Some(r#"{"properties":{"a":{"type":"keyword"}}}"#)
    );
    let given = inherit(
        request(1, Some(r#"{"mappings":{"dynamic":"strict","properties":{"a":{"type":"text"},"b":{"type":"long"}}}}"#)),
        &inherited,
    )
    .unwrap();
    assert_eq!(
        given.mapping.as_deref(),
        Some(r#"{"properties":{"a":{"type":"text"},"b":{"type":"long"}},"dynamic":"strict"}"#)
    );
    // A mapping given as the whole body, or wrapped in `_doc`, merges the same.
    let bare = inherit(
        request(1, Some(r#"{"properties":{"b":{"type":"long"}}}"#)),
        &inherited,
    )
    .unwrap();
    assert_eq!(
        bare.mapping.as_deref(),
        Some(r#"{"properties":{"a":{"type":"keyword"},"b":{"type":"long"}}}"#)
    );
    let wrapped = inherit(
        request(
            1,
            Some(r#"{"mappings":{"_doc":{"properties":{"b":{"type":"long"}}}}}"#),
        ),
        &inherited,
    )
    .unwrap();
    assert_eq!(wrapped.mapping, bare.mapping);
}

#[test]
fn a_mapping_that_is_not_one_object_is_refused() {
    let inherited = from_template(ALL);
    for body in [
        r#"{"mappings":{"_doc":{"properties":{}},"other":{}}}"#,
        r#"{"mappings":{"_doc":5}}"#,
    ] {
        let err = inherit(request(1, Some(body)), &inherited).unwrap_err();
        assert_eq!((err.status, err.kind), (400, "malformed_body"), "{body}");
    }
}

#[test]
fn a_request_without_mappings_inherits_them_whole_and_aliases_pass_through() {
    let inherited = from_template(ALL);
    let given = request(1, Some(r#"{"aliases":{"a1":{}}}"#));
    let got = inherit(given.clone(), &inherited).unwrap();
    assert_eq!(got.aliases, given.aliases);
    assert!(got.mapping.is_some());
    // Template settings only: mappings stay as the request had them.
    let settings_only = from_template(
        r#"{"index_patterns":["*"],"template":{"settings":{"index":{"refresh_interval":"9s"}}}}"#,
    );
    let kept = inherit(
        request(1, Some(r#"{"mappings":{"properties":{}}}"#)),
        &settings_only,
    )
    .unwrap();
    assert_eq!(kept.mapping.as_deref(), Some(r#"{"properties":{}}"#));
}
