//! RFC 6902 patches of security documents and the list answer
//! (docs/specs/security-complete/SPEC.md, slice 1).

use oss_coordinator::security::*;
use oss_doc::mapping::json::parse;
use serde_json::{Value, json};

fn doc(text: &str) -> oss_doc::mapping::json::Json {
    parse(text.as_bytes()).expect("json")
}

fn patched(kind: Kind, stored: &str, ops: &str) -> Result<Value, (u16, Value)> {
    patch_document(kind, doc(stored), ops.as_bytes())
        .map(|d| serde_json::from_str(&d.to_json()).unwrap())
        .map_err(|a| (a.status, serde_json::from_str(&a.body).unwrap()))
}

#[test]
fn patch_applies_each_operation() {
    let stored = r#"{"cluster_permissions":["a"],"description":"d"}"#;
    assert_eq!(
        patched(
            Kind::Roles,
            stored,
            r#"[{"op":"add","path":"/cluster_permissions/-","value":"b"},
                {"op":"replace","path":"/description","value":"e"},
                {"op":"add","path":"/cluster_permissions/0","value":"z"}]"#
        ),
        Ok(json!({"cluster_permissions":["z","a","b"],"description":"e"}))
    );
    assert_eq!(
        patched(
            Kind::Roles,
            stored,
            r#"[{"op":"remove","path":"/description"}]"#
        ),
        Ok(json!({"cluster_permissions":["a"]}))
    );
    assert_eq!(
        patched(
            Kind::Roles,
            stored,
            r#"[{"op":"remove","path":"/cluster_permissions/0"}]"#
        ),
        Ok(json!({"cluster_permissions":[],"description":"d"}))
    );
    // `~1` is `/` and `~0` is `~` in a path.
    assert_eq!(
        patched(
            Kind::InternalUsers,
            r#"{"attributes":{}}"#,
            r#"[{"op":"add","path":"/attributes/a~1b~0c","value":"v"}]"#
        ),
        Ok(json!({"attributes":{"a/b~c":"v"}}))
    );
}

#[test]
fn a_patch_of_the_wrong_shape_is_400() {
    let bad = |ops: &str| {
        let (status, body) = patched(Kind::Tenants, r#"{"description":"d"}"#, ops).unwrap_err();
        assert_eq!(status, 400, "{ops}: {body}");
        assert_eq!(body["status"], "BAD_REQUEST", "{ops}");
    };
    bad("");
    bad("{}");
    bad(r#"[{"op":"test","path":"/description","value":"d"}]"#);
    bad(r#"{"op":"add"}"#);
    bad(r#"[{"op":"frob","path":"/description","value":"x"}]"#);
    bad(r#"[{"op":"move","from":"/a","path":"/b"}]"#);
    bad(r#"[{"op":"add","value":"x"}]"#);
    bad(r#"[{"op":"add","path":"/description"}]"#);
    bad(r#"[{"op":"remove","path":"/missing"}]"#);
    bad(r#"[{"op":"replace","path":"/missing","value":"x"}]"#);
    bad(r#"[{"op":"add","path":"description","value":"x"}]"#);
    bad(r#"[{"op":"add","path":"/a/b/c","value":"x"}]"#);
}

#[test]
fn a_patch_that_makes_an_invalid_document_is_refused_as_a_put_would() {
    let (status, body) = patched(
        Kind::Tenants,
        r#"{"description":"d"}"#,
        r#"[{"op":"add","path":"/bogus","value":1}]"#,
    )
    .unwrap_err();
    assert_eq!(status, 400);
    assert_eq!(body["reason"], "Invalid configuration");
    assert_eq!(body["invalid_keys"]["keys"], "bogus");
    let (status, body) = patched(
        Kind::Tenants,
        r#"{"description":"d"}"#,
        r#"[{"op":"replace","path":"/description","value":5}]"#,
    )
    .unwrap_err();
    assert_eq!((status, &body["reason"]), (400, &json!("Wrong datatype")));
}

#[test]
fn a_bulk_patch_is_split_by_document_name() {
    let plan = plan_bulk(
        br#"[{"op":"add","path":"/new","value":{"description":"x"}},
             {"op":"replace","path":"/old/description","value":"y"},
             {"op":"remove","path":"/gone"},
             {"op":"add","path":"/old/extra","value":1}]"#,
    )
    .expect("plans");
    let names: Vec<&str> = plan.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["new", "old", "gone"], "first-seen order");
    assert_eq!(plan[1].1.len(), 2);
    // A whole-document add creates; a whole-document remove deletes.
    let created = apply_ops(None, &plan[0].1).expect("applies");
    assert_eq!(
        created.map(|d| d.to_json()),
        Some(r#"{"description":"x"}"#.to_owned())
    );
    let existing = doc(r#"{"description":"d"}"#);
    assert_eq!(
        apply_ops(Some(existing), &plan[2].1).expect("applies"),
        None
    );
    // Anything but an array of operations with a name first is 400.
    for bad in [
        "",
        "{}",
        r#"[{"op":"add","path":"","value":{}}]"#,
        r#"[{"op":"add","path":"name","value":{}}]"#,
    ] {
        assert_eq!(plan_bulk(bad.as_bytes()).unwrap_err().status, 400, "{bad}");
    }
    // Removing a document that is not there changes nothing.
    let nothing = apply_ops(
        None,
        &plan_bulk(br#"[{"op":"remove","path":"/ghost"}]"#).unwrap()[0].1,
    );
    assert_eq!(nothing.expect("applies"), None);
    // An operation on a document that is not there is 404-shaped, not a create.
    let missing = apply_ops(
        None,
        &plan_bulk(br#"[{"op":"replace","path":"/ghost/description","value":"x"}]"#).unwrap()[0].1,
    )
    .unwrap_err();
    assert_eq!(missing.status, 404);
}

#[test]
fn a_list_holds_every_document_as_a_read_renders_it() {
    let entries = vec![
        ("a".to_owned(), doc(r#"{"description":"one"}"#)),
        ("b".to_owned(), doc(r#"{}"#)),
    ];
    let listed: Value = serde_json::from_str(&render_list(Kind::Tenants, &entries)).unwrap();
    for (name, d) in &entries {
        let one: Value = serde_json::from_str(&render(Kind::Tenants, name, d)).unwrap();
        assert_eq!(listed[name], one[name], "{name}");
    }
    assert_eq!(listed.as_object().unwrap().len(), 2);
}

#[test]
fn an_empty_kind_lists_as_an_empty_object() {
    assert_eq!(render_list(Kind::Roles, &[]), "{}");
}

#[test]
fn the_recorded_messages_are_the_plugins() {
    let message = |kind: Kind, stored: &str, ops: &str| {
        patched(kind, stored, ops).unwrap_err().1["message"].clone()
    };
    let t = r#"{"description":"d"}"#;
    assert_eq!(
        message(Kind::Tenants, t, r#"[{"op":"nope","path":"/x","value":1}]"#),
        json!("Unsupported patch operation: nope. Supported are: add,remove,replace")
    );
    assert_eq!(
        message(
            Kind::Tenants,
            t,
            r#"[{"op":"replace","path":"/nope","value":1}]"#
        ),
        json!("Missing field \"nope\"")
    );
    assert_eq!(
        message(
            Kind::Tenants,
            t,
            r#"{"op":"replace","path":"/description","value":""}"#
        ),
        json!("Wrong request body")
    );
    assert_eq!(
        message(
            Kind::Roles,
            r#"{"index_permissions":[]}"#,
            r#"[{"op":"replace","path":"/index_permissions/7/allowed_actions","value":["read"]}]"#
        ),
        json!("Array index 7 is out of bounds")
    );
}
