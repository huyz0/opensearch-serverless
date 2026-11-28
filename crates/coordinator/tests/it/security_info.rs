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
