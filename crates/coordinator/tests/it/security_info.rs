//! The configuration and information answers (docs/specs/security-complete/SPEC.md, slice 3).

use oss_coordinator::security::{
    Config, access_denied, cache_flushed, permissionsinfo, securityconfig,
};
use serde_json::{Value, json};

#[test]
fn securityconfig_is_json_with_basic_as_the_only_domain() {
    let v: Value = serde_json::from_str(securityconfig()).expect("json");
    let authc = v["config"]["dynamic"]["authc"].as_object().expect("authc");
    assert_eq!(authc.len(), 1);
    assert_eq!(authc["basic_internal_auth_domain"]["order"], 4);
    assert_eq!(
        v["config"]["dynamic"]["kibana"]["multitenancy_enabled"],
        false
    );
}

#[test]
fn permissionsinfo_names_the_user_and_their_backend_roles() {
    let config = Config::new().with_admin("admin", "$2b$04$adminhash");
    let got: Value = serde_json::from_str(&permissionsinfo(&config, "admin")).expect("json");
    assert_eq!(
        got,
        json!({
            "user": "User [name=admin, backend_roles=[admin], requestedTenant=null]",
            "user_name": "admin",
            "has_api_access": true,
            "disabled_endpoints": {}
        })
    );
    let unknown: Value = serde_json::from_str(&permissionsinfo(&config, "ghost")).expect("json");
    assert_eq!(
        unknown["user"],
        "User [name=ghost, backend_roles=[], requestedTenant=null]"
    );
}

#[test]
fn the_fixed_answers_are_the_recorded_ones() {
    let denied = access_denied();
    assert_eq!(denied.status, 403);
    assert_eq!(
        denied.body,
        r#"{"status":"FORBIDDEN","message":"Access denied"}"#
    );
    let flushed = cache_flushed();
    assert_eq!(flushed.status, 200);
    assert!(flushed.body.contains("Cache flushed successfully."));
}

#[test]
fn a_password_change_needs_both_strings() {
    use oss_coordinator::security::{account_reserved, password_change, wrong_current};
    assert_eq!(
        password_change(br#"{"current_password":"a","password":"b"}"#),
        Ok(("a".to_owned(), "b".to_owned()))
    );
    for bad in [
        &b""[..],
        b"[]",
        br#"{"password":"b"}"#,
        br#"{"current_password":"a"}"#,
    ] {
        assert_eq!(password_change(bad).expect_err("refused").status, 400);
    }
    assert_eq!(wrong_current().status, 400);
    assert!(wrong_current().body.contains("current password"));
    assert_eq!(account_reserved("admin").status, 403);
    assert!(
        account_reserved("admin")
            .body
            .contains("'admin' is reserved.")
    );
}

#[test]
fn the_account_of_a_user_and_of_the_administrator() {
    use oss_coordinator::security::account;
    let config = Config::new().with_admin("admin", "$2b$04$adminhash");
    let got: Value = serde_json::from_str(&account(&config, "admin")).expect("json");
    assert_eq!(
        got,
        json!({
            "user_name": "admin",
            "is_reserved": true,
            "is_hidden": false,
            "is_internal_user": true,
            "user_requested_tenant": null,
            "backend_roles": ["admin"],
            "custom_attribute_names": [],
            "tenants": {"admin": true, "admin_tenant": true, "global_tenant": true},
            "roles": ["all_access"]
        })
    );
    let unknown: Value = serde_json::from_str(&account(&config, "ghost")).expect("json");
    assert_eq!(unknown["is_reserved"], false);
    assert_eq!(unknown["tenants"], json!({"ghost": true}));
    assert_eq!(unknown["roles"], json!([]));
}
