//! Credentials, roles and `authinfo` (docs/specs/security-authn/SPEC.md).

use oss_coordinator::security::{ALL_ACCESS, Config, Kind, authinfo, basic};
use oss_doc::mapping::json::parse;
use serde_json::{Value, json};

fn doc(text: &str) -> oss_doc::mapping::json::Json {
    parse(text.as_bytes()).expect("json")
}

#[test]
fn a_basic_header_is_a_user_and_a_password_and_nothing_else_is() {
    assert_eq!(
        basic("Basic YWxpY2U6cHc="),
        Some(("alice".to_owned(), "pw".to_owned()))
    );
    assert_eq!(
        basic("basic YWxpY2U6cDp3"),
        Some(("alice".to_owned(), "p:w".to_owned())),
        "a password may hold a colon; the scheme is case-insensitive"
    );
    assert_eq!(
        basic("Basic YWxpY2U6"),
        Some(("alice".to_owned(), String::new()))
    );
    for bad in [
        "Bearer abc",
        "Basic",
        "Basic !!!!",
        "Basic bm9jb2xvbg==",
        "Basic OnB3",
        "Basic /w==",
        &format!("Basic {}", "QQ==".repeat(400)),
    ] {
        assert_eq!(basic(bad), None, "{bad}");
    }
}

fn config() -> Config {
    let mut c = Config::new().with_admin("admin", "$2b$04$adminhash");
    c.add(
        Kind::InternalUsers,
        "u",
        &doc(r#"{"hash":"h","backend_roles":["b1","b2"],"opendistro_security_roles":["own"],"attributes":{"k":"v"}}"#),
    );
    c.add(Kind::InternalUsers, "admin", &doc(r#"{"hash":"shadowed"}"#));
    c.add(Kind::RolesMapping, "by_name", &doc(r#"{"users":["u"]}"#));
    c.add(
        Kind::RolesMapping,
        "by_backend",
        &doc(r#"{"backend_roles":["b2"]}"#),
    );
    c.add(
        Kind::RolesMapping,
        "all_of",
        &doc(r#"{"and_backend_roles":["b1","b2"]}"#),
    );
    c.add(
        Kind::RolesMapping,
        "not_all",
        &doc(r#"{"and_backend_roles":["b1","zz"]}"#),
    );
    c.add(
        Kind::RolesMapping,
        "other",
        &doc(r#"{"users":["x"],"backend_roles":["y"]}"#),
    );
    c.add(Kind::RolesMapping, "everyone", &doc(r#"{"users":["*"]}"#));
    c.add(Kind::Tenants, "t1", &doc("{}"));
    c
}

#[test]
fn a_user_holds_the_roles_its_mappings_name() {
    let c = config();
    assert_eq!(
        c.roles_of("u"),
        ["all_of", "by_backend", "by_name", "everyone", "own"]
    );
    assert_eq!(c.roles_of("admin"), [ALL_ACCESS, "everyone"]);
    assert!(c.roles_of("nobody").is_empty());
    assert_eq!(
        c.user("admin").map(|u| u.hash.as_str()),
        Some("$2b$04$adminhash")
    );
}

#[test]
fn authinfo_is_the_recorded_shape() {
    let c = config();
    let v: Value = serde_json::from_str(&authinfo(&c, "u")).expect("json");
    assert_eq!(
        v,
        json!({
            "user": "User [name=u, backend_roles=[b1, b2], requestedTenant=null]",
            "user_name": "u",
            "user_requested_tenant": null,
            "remote_address": null,
            "backend_roles": ["b1", "b2"],
            "custom_attribute_names": ["attr.internal.k"],
            "roles": ["all_of", "by_backend", "by_name", "everyone", "own"],
            "tenants": {"u": true},
            "principal": null,
            "peer_certificates": "0",
            "sso_logout_url": null
        })
    );
    let admin: Value = serde_json::from_str(&authinfo(&c, "admin")).expect("json");
    assert_eq!(
        admin["tenants"],
        json!({"admin": true, "admin_tenant": true, "global_tenant": true, "t1": true})
    );
}
