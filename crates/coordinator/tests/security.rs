//! What the security configuration API validates and answers
//! (docs/specs/security-config/SPEC.md).

use oss_coordinator::security::*;
use oss_doc::mapping::json::parse;
use serde_json::{Value, json};

fn answer(a: &Answer) -> (u16, Value) {
    (a.status, serde_json::from_str(&a.body).unwrap())
}

fn check(kind: Kind, body: &str) -> Result<(), (u16, Value)> {
    validate(kind, body.as_bytes())
        .map(|_| ())
        .map_err(|a| answer(&a))
}

#[test]
fn a_valid_body_for_each_kind_is_accepted() {
    for (kind, body) in [
        (
            Kind::Roles,
            r#"{"cluster_permissions":["a"],"index_permissions":[{"index_patterns":["x*"],"allowed_actions":["read"],"dls":"{}"}],"tenant_permissions":[{"tenant_patterns":["t"],"allowed_actions":["a"]}],"description":"d"}"#,
        ),
        (Kind::RolesMapping, r#"{"users":["u"]}"#),
        (
            Kind::InternalUsers,
            r#"{"password":"abcdefgh","attributes":{"k":"v"},"backend_roles":["b"]}"#,
        ),
        (Kind::Tenants, r#"{"description":"d"}"#),
        (
            Kind::ActionGroups,
            r#"{"allowed_actions":["a"],"type":"index"}"#,
        ),
    ] {
        assert_eq!(check(kind, body), Ok(()), "{kind:?}");
    }
}

#[test]
fn an_unknown_missing_or_unspecified_key_is_the_plugins_invalid_configuration() {
    assert_eq!(
        check(Kind::Roles, r#"{"bogus":1,"other":2}"#),
        Err((
            400,
            json!({"status":"error","reason":"Invalid configuration","invalid_keys":{"keys":"bogus,other"}})
        ))
    );
    assert_eq!(
        check(Kind::ActionGroups, r#"{"bogus":1}"#),
        Err((
            400,
            json!({"status":"error","reason":"Invalid configuration","invalid_keys":{"keys":"bogus"},"missing_mandatory_keys":{"keys":"allowed_actions"}})
        ))
    );
    assert_eq!(
        check(Kind::RolesMapping, r#"{"bogus":1}"#),
        Err((
            400,
            json!({"status":"error","reason":"Invalid configuration","invalid_keys":{"keys":"bogus"},"specify_one_of":{"keys":"backend_roles,hosts,and_backend_roles,users"}})
        ))
    );
    assert_eq!(
        check(Kind::Roles, r#"{"index_permissions":[{"bogus":1}]}"#),
        Err((
            400,
            json!({"status":"error","reason":"Invalid configuration","invalid_keys":{"keys":"bogus"}})
        ))
    );
}

#[test]
fn a_value_of_the_wrong_type_names_its_key() {
    for (kind, body, key, expected) in [
        (
            Kind::Roles,
            r#"{"cluster_permissions":"x"}"#,
            "cluster_permissions",
            "Array expected",
        ),
        (
            Kind::Roles,
            r#"{"cluster_permissions":[1]}"#,
            "cluster_permissions",
            "Array expected",
        ),
        (
            Kind::Roles,
            r#"{"description":3}"#,
            "description",
            "String expected",
        ),
        (
            Kind::InternalUsers,
            r#"{"password":"x","attributes":[]}"#,
            "attributes",
            "Object expected",
        ),
        (
            Kind::Roles,
            r#"{"index_permissions":[3]}"#,
            "index_permissions",
            "Object expected",
        ),
        (
            Kind::Roles,
            r#"{"index_permissions":{}}"#,
            "index_permissions",
            "Array expected",
        ),
        (
            Kind::Roles,
            r#"{"index_permissions":[{"dls":1}]}"#,
            "dls",
            "String expected",
        ),
    ] {
        assert_eq!(
            check(kind, body),
            Err((
                400,
                json!({"status":"error","reason":"Wrong datatype", key: expected})
            )),
            "{body}"
        );
    }
}

#[test]
fn an_empty_or_unparseable_body_is_refused() {
    assert_eq!(
        check(Kind::Roles, "  "),
        Err((
            400,
            json!({"status":"error","reason":"Request body required for this action."})
        ))
    );
    for body in ["nojson", "[1]", "3", r#"{"a":1} x"#] {
        assert_eq!(
            check(Kind::Roles, body),
            Err((
                400,
                json!({"status":"error","reason":"Could not parse content of request."})
            )),
            "{body}"
        );
    }
}

#[test]
fn acknowledgements_and_absences_read_as_the_plugin_words_them() {
    assert_eq!(
        answer(&saved("r", true)),
        (201, json!({"status":"CREATED","message":"'r' created."}))
    );
    assert_eq!(
        answer(&saved("r", false)),
        (200, json!({"status":"OK","message":"'r' updated."}))
    );
    assert_eq!(
        answer(&deleted("r")),
        (200, json!({"status":"OK","message":"'r' deleted."}))
    );
    for (kind, noun) in [
        (Kind::Roles, "role"),
        (Kind::RolesMapping, "rolesmapping"),
        (Kind::InternalUsers, "user"),
        (Kind::Tenants, "tenant"),
        (Kind::ActionGroups, "actiongroup"),
    ] {
        assert_eq!(
            answer(&missing(kind, "n\"x")),
            (
                404,
                json!({"status":"NOT_FOUND","message": format!("{noun} 'n\"x' not found.")})
            )
        );
    }
}

fn shown(kind: Kind, body: &str) -> Value {
    let doc = validate(kind, body.as_bytes()).unwrap();
    serde_json::from_str(&render(kind, "n", &doc)).unwrap()
}

#[test]
fn a_read_adds_the_defaults_the_plugin_adds() {
    assert_eq!(
        shown(
            Kind::Roles,
            r#"{"cluster_permissions":["m"],"index_permissions":[{"index_patterns":["a*"],"allowed_actions":["read"]}]}"#
        ),
        json!({"n":{"cluster_permissions":["m"],"hidden":false,"index_permissions":[{"allowed_actions":["read"],"fls":[],"index_patterns":["a*"],"masked_fields":[]}],"reserved":false,"static":false,"tenant_permissions":[]}})
    );
    assert_eq!(
        shown(Kind::RolesMapping, r#"{"users":["u"]}"#),
        json!({"n":{"and_backend_roles":[],"backend_roles":[],"hidden":false,"hosts":[],"reserved":false,"users":["u"]}})
    );
    assert_eq!(
        shown(Kind::Tenants, r#"{"description":"d"}"#),
        json!({"n":{"description":"d","hidden":false,"reserved":false,"static":false}})
    );
    assert_eq!(
        shown(
            Kind::ActionGroups,
            r#"{"allowed_actions":["a"],"type":"index","description":"x"}"#
        ),
        json!({"n":{"allowed_actions":["a"],"description":"x","hidden":false,"reserved":false,"static":false,"type":"index"}})
    );
}

#[test]
fn an_answer_never_carries_a_hash_or_a_password() {
    let doc = validate(
        Kind::InternalUsers,
        br#"{"password":"abcdefgh","backend_roles":["b"]}"#,
    )
    .unwrap();
    let stored = with_hash(doc, Some("$2b$12$hashhashhash".to_owned()));
    assert!(stored.get("password").is_none());
    assert_eq!(
        stored.get("hash"),
        Some(&oss_doc::mapping::json::Json::Str(
            "$2b$12$hashhashhash".into()
        ))
    );
    let shown: Value = serde_json::from_str(&render(Kind::InternalUsers, "u", &stored)).unwrap();
    assert_eq!(
        shown,
        json!({"u":{"attributes":{},"backend_roles":["b"],"hash":"","hidden":false,"opendistro_security_roles":[],"reserved":false,"static":false}})
    );
    assert!(!render(Kind::InternalUsers, "u", &stored).contains("abcdefgh"));
    // A body kept as the user sent it: no password means the stored hash stays with the caller.
    assert!(
        with_hash(parse(b"{\"hash\":\"h\"}").unwrap(), None)
            .get("hash")
            .is_some()
    );
}

#[test]
fn a_password_policy_and_the_missing_credentials_answer() {
    assert!(password_ok("abcdefgh") && !password_ok("short"));
    assert_eq!(
        answer(&weak_password()),
        (400, json!({"status":"error","reason":"Weak password"}))
    );
    assert_eq!(answer(&needs_credentials()).0, 400);
}
