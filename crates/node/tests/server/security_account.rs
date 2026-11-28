//! The account API (docs/specs/security-complete/SPEC.md, slice 4).

use crate::security_authn::{ADMIN, API, Rig, call, make_user, rig};

use serde_json::{Value, json};

const USER: &str = "u:Zz9!longEnough";

fn account(r: &Rig, auth: &str, method: &str, body: Option<&str>) -> (u16, String) {
    call(r, Some(auth), method, &format!("{API}/account"), body)
}

#[test]
fn account_answers_the_recorded_shape() {
    let r = rig();
    let role = call(
        &r,
        Some(ADMIN),
        "PUT",
        &format!("{API}/roles/r1"),
        Some(r#"{"cluster_permissions":["cluster_monitor"]}"#),
    );
    assert!(role.0 == 200 || role.0 == 201, "{role:?}");
    let user = call(
        &r,
        Some(ADMIN),
        "PUT",
        &format!("{API}/internalusers/u"),
        Some(
            r#"{"password":"Zz9!longEnough","backend_roles":["b"],"opendistro_security_roles":["r1"],"attributes":{"a":"1"}}"#,
        ),
    );
    assert!(user.0 == 200 || user.0 == 201, "{user:?}");
    let got = account(&r, USER, "GET", None);
    assert_eq!(got.0, 200, "{}", got.1);
    let body: Value = serde_json::from_str(&got.1).expect("json");
    assert_eq!(
        body,
        json!({
            "user_name": "u",
            "is_reserved": false,
            "is_hidden": false,
            "is_internal_user": true,
            "user_requested_tenant": null,
            "backend_roles": ["b"],
            "custom_attribute_names": ["attr.internal.a"],
            "tenants": {"u": true},
            "roles": ["r1"]
        })
    );
}

#[test]
fn the_account_of_the_administrator_is_reserved() {
    let r = rig();
    let got = account(&r, ADMIN, "GET", None);
    assert_eq!(got.0, 200, "{}", got.1);
    let body: Value = serde_json::from_str(&got.1).expect("json");
    assert_eq!(body["user_name"], "admin");
    assert_eq!(body["is_reserved"], true);
    assert_eq!(body["roles"], json!(["all_access"]));
    assert_eq!(body["tenants"]["global_tenant"], true);
}

#[test]
fn a_user_without_admin_access_reads_its_own_account() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    assert_eq!(account(&r, USER, "GET", None).0, 200);
    let put = account(
        &r,
        USER,
        "PUT",
        Some(r#"{"current_password":"Zz9!longEnough","password":"New-Pass-7y!q"}"#),
    );
    assert_eq!(put.0, 200, "{put:?}");
}

#[test]
fn a_user_changes_its_own_password() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    let put = account(
        &r,
        USER,
        "PUT",
        Some(r#"{"current_password":"Zz9!longEnough","password":"New-Pass-7y!q"}"#),
    );
    assert_eq!(
        put,
        (
            200,
            r#"{"status":"OK","message":"'u' updated."}"#.to_owned()
        )
    );
    // The change is visible to the next request.
    assert_eq!(account(&r, USER, "GET", None).0, 401);
    assert_eq!(account(&r, "u:New-Pass-7y!q", "GET", None).0, 200);
}

#[test]
fn a_wrong_current_password_changes_nothing() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    let wrong = account(
        &r,
        USER,
        "PUT",
        Some(r#"{"current_password":"nope","password":"New-Pass-7y!q"}"#),
    );
    assert_eq!(
        wrong,
        (
            400,
            r#"{"status":"BAD_REQUEST","message":"Could not validate your current password."}"#
                .to_owned()
        )
    );
    let weak = account(
        &r,
        USER,
        "PUT",
        Some(r#"{"current_password":"Zz9!longEnough","password":"x"}"#),
    );
    assert_eq!(weak.0, 400, "{weak:?}");
    assert!(weak.1.contains("Weak password"), "{weak:?}");
    assert_eq!(account(&r, USER, "GET", None).0, 200, "still the old one");
}

#[test]
fn a_malformed_password_change_is_400() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    for body in [
        "",
        "[]",
        "not json",
        r#"{"password":"New-Pass-7y!q"}"#,
        r#"{"current_password":"Zz9!longEnough"}"#,
        r#"{"current_password":1,"password":"New-Pass-7y!q"}"#,
    ] {
        let got = account(&r, USER, "PUT", Some(body));
        assert_eq!(got.0, 400, "{body}: {got:?}");
    }
    assert_eq!(account(&r, USER, "GET", None).0, 200);
}

#[test]
fn the_administrator_cannot_change_a_stored_password() {
    let r = rig();
    let got = account(
        &r,
        ADMIN,
        "PUT",
        Some(r#"{"current_password":"Adm1n!Password","password":"New-Pass-7y!q"}"#),
    );
    assert_eq!(
        got,
        (
            403,
            r#"{"status":"FORBIDDEN","message":"Resource 'admin' is reserved."}"#.to_owned()
        )
    );
    assert_eq!(account(&r, ADMIN, "GET", None).0, 200);
}
