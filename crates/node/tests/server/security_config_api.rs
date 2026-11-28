//! `securityconfig`, `permissionsinfo`, the closed routes and the cache
//! flush (docs/specs/security-complete/SPEC.md, slice 3).

use crate::security_authn::{ADMIN, API, make_user, rig};

use oss_blob::{BlobStore, S3Op};
use serde_json::Value;

fn get(
    r: &crate::security_authn::Rig,
    auth: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, String) {
    crate::security_authn::call(r, Some(auth), method, &format!("{API}{path}"), body)
}

#[test]
fn securityconfig_answers_the_effective_configuration() {
    let r = rig();
    let got = get(&r, ADMIN, "GET", "/securityconfig", None);
    assert_eq!(got.0, 200, "{}", got.1);
    let body: Value = serde_json::from_str(&got.1).expect("json");
    let dynamic = &body["config"]["dynamic"];
    let authc = dynamic["authc"].as_object().expect("authc");
    assert_eq!(
        authc.keys().collect::<Vec<_>>(),
        ["basic_internal_auth_domain"]
    );
    assert_eq!(authc["basic_internal_auth_domain"]["http_enabled"], true);
    assert_eq!(dynamic["kibana"]["multitenancy_enabled"], false);
    // The keys the recorded 3.10.0 answer has.
    let recorded: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/compat/security-api/security-api.json"
    ))
    .expect("recording");
    let step = recorded["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|s| s["name"] == "securityconfig")
        .expect("step");
    let want: Vec<&String> = step["response"]["body"]["config"]["dynamic"]
        .as_object()
        .expect("dynamic")
        .keys()
        .collect();
    let mut have: Vec<&String> = dynamic.as_object().expect("dynamic").keys().collect();
    let mut want = want;
    have.sort();
    want.sort();
    assert_eq!(have, want);
}

#[test]
fn the_configuration_that_stock_keeps_closed_is_403_and_makes_no_request() {
    let r = rig();
    // The first request refreshes the authentication snapshot; the routes add nothing to it.
    get(&r, ADMIN, "GET", "/permissionsinfo", None);
    let before: u64 = S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum();
    for (method, path, body) in [
        ("PUT", "/securityconfig/config", Some(r#"{"dynamic":{}}"#)),
        (
            "PATCH",
            "/securityconfig",
            Some(r#"[{"op":"add","path":"/config/dynamic/nope","value":1}]"#),
        ),
        ("GET", "/allowlist", None),
        ("PUT", "/allowlist", Some(r#"{"enabled":false}"#)),
        ("GET", "/ssl/certs", None),
    ] {
        let got = get(&r, ADMIN, method, path, body);
        assert_eq!(
            got,
            (
                403,
                r#"{"status":"FORBIDDEN","message":"Access denied"}"#.to_owned()
            ),
            "{method} {path}"
        );
    }
    let after: u64 = S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum();
    assert_eq!(after - before, 0, "a closed route reads nothing");
}

#[test]
fn permissionsinfo_names_the_caller() {
    let r = rig();
    let got = get(&r, ADMIN, "GET", "/permissionsinfo", None);
    assert_eq!(got.0, 200, "{}", got.1);
    let body: Value = serde_json::from_str(&got.1).expect("json");
    assert_eq!(body["user_name"], "admin");
    assert_eq!(
        body["user"],
        "User [name=admin, backend_roles=[admin], requestedTenant=null]"
    );
    assert_eq!(body["has_api_access"], true);
    assert_eq!(body["disabled_endpoints"], serde_json::json!({}));
}

#[test]
fn a_user_without_admin_access_is_refused_every_one_of_these_routes() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    for (method, path) in [
        ("GET", "/securityconfig"),
        ("PUT", "/securityconfig/config"),
        ("GET", "/allowlist"),
        ("GET", "/ssl/certs"),
        ("GET", "/permissionsinfo"),
        ("DELETE", "/cache"),
    ] {
        let got = get(&r, "u:Zz9!longEnough", method, path, Some("{}"));
        assert_eq!(got.0, 403, "{method} {path}: {}", got.1);
        assert!(
            got.1.contains("No permission to access REST API"),
            "{method} {path}: {}",
            got.1
        );
    }
}

#[test]
fn flushing_the_cache_makes_the_next_request_read_the_sets() {
    let r = rig();
    let reads = |r: &crate::security_authn::Rig| -> u64 {
        [S3Op::Get, S3Op::GetRange, S3Op::Head]
            .iter()
            .map(|op| r.store.meter().count(*op))
            .sum()
    };
    get(&r, ADMIN, "GET", "/permissionsinfo", None);
    let settled = reads(&r);
    get(&r, ADMIN, "GET", "/permissionsinfo", None);
    assert_eq!(reads(&r), settled, "inside the interval nothing is read");
    let flushed = get(&r, ADMIN, "DELETE", "/cache", None);
    assert_eq!(
        flushed,
        (
            200,
            r#"{"status":"OK","message":"Cache flushed successfully."}"#.to_owned()
        )
    );
    get(&r, ADMIN, "GET", "/permissionsinfo", None);
    assert!(reads(&r) > settled, "the snapshot was read again");
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}
