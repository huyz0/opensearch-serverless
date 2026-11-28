//! The documents the plugin ships, over a node
//! (docs/specs/security-complete/SPEC.md, slice 2).

use crate::security_route::{API, call, rig, total};

use oss_blob::S3Op;
use serde_json::Value;

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("json")
}

#[test]
fn a_list_holds_the_built_in_documents_with_their_flags() {
    let r = rig();
    call(&r, "PUT", &format!("{API}/roles/mine"), Some("{}"));
    let roles = json(&call(&r, "GET", &format!("{API}/roles"), None).1);
    assert_eq!(roles["mine"]["reserved"], false);
    assert_eq!(roles["kibana_user"]["static"], true);
    assert_eq!(roles["kibana_user"]["reserved"], true);
    assert_eq!(roles["security_rest_api_access"]["static"], false);
    assert_eq!(roles["security_rest_api_access"]["reserved"], true);
    assert_eq!(roles.as_object().expect("object").len(), 58);
    let names: Vec<&String> = roles.as_object().expect("object").keys().collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "name order");
    let groups = json(&call(&r, "GET", &format!("{API}/actiongroups"), None).1);
    assert_eq!(groups["read"]["static"], true);
    let tenants = json(&call(&r, "GET", &format!("{API}/tenants"), None).1);
    assert_eq!(tenants["global_tenant"]["reserved"], true);
    assert_eq!(tenants.as_object().expect("object").len(), 1);
}

#[test]
fn a_built_in_is_read_by_name_without_a_store_request() {
    let r = rig();
    let before = total(&r, &[S3Op::Get, S3Op::GetRange, S3Op::Head, S3Op::List]);
    let read = call(&r, "GET", &format!("{API}/roles/kibana_user"), None);
    assert_eq!(read.0, 200, "{}", read.1);
    assert_eq!(json(&read.1)["kibana_user"]["static"], true);
    assert_eq!(
        total(&r, &[S3Op::Get, S3Op::GetRange, S3Op::Head, S3Op::List]),
        before
    );
}

#[test]
fn a_built_in_cannot_be_written_patched_or_deleted() {
    let r = rig();
    let puts = total(&r, &[S3Op::Put, S3Op::Delete]);
    for (path, status, message) in [
        (
            "roles/kibana_user",
            403,
            "Resource 'kibana_user' is static.",
        ),
        ("actiongroups/read", 403, "Resource 'read' is static."),
        (
            "tenants/global_tenant",
            403,
            "Resource 'global_tenant' is static.",
        ),
        (
            "roles/security_rest_api_access",
            403,
            "Resource 'security_rest_api_access' is reserved.",
        ),
    ] {
        for (method, body) in [
            ("PUT", Some("{}")),
            (
                "PATCH",
                Some(r#"[{"op":"add","path":"/description","value":"x"}]"#),
            ),
            ("DELETE", None),
        ] {
            let got = call(&r, method, &format!("{API}/{path}"), body);
            assert_eq!(got.0, status, "{method} {path}: {}", got.1);
            assert_eq!(json(&got.1)["message"], message, "{method} {path}");
        }
    }
    assert_eq!(
        total(&r, &[S3Op::Put, S3Op::Delete]),
        puts,
        "nothing written"
    );
}

#[test]
fn a_bulk_patch_naming_a_built_in_writes_nothing() {
    let r = rig();
    let puts = total(&r, &[S3Op::Put, S3Op::Delete]);
    let got = call(
        &r,
        "PATCH",
        &format!("{API}/roles"),
        Some(r#"[{"op":"add","path":"/fresh","value":{}},{"op":"remove","path":"/kibana_user"}]"#),
    );
    assert_eq!(got.0, 403, "{}", got.1);
    assert_eq!(total(&r, &[S3Op::Put, S3Op::Delete]), puts);
    assert_eq!(call(&r, "GET", &format!("{API}/roles/fresh"), None).0, 404);
}

#[test]
fn a_mapping_for_a_built_in_role_is_accepted() {
    let r = rig();
    let put = call(
        &r,
        "PUT",
        &format!("{API}/rolesmapping/kibana_user"),
        Some(r#"{"users":["u"]}"#),
    );
    assert_eq!(put.0, 201, "{}", put.1);
}

#[test]
fn no_demo_user_is_shipped() {
    let r = rig();
    assert_eq!(
        call(&r, "GET", &format!("{API}/internalusers"), None),
        (200, "{}".to_owned())
    );
    assert_eq!(
        call(&r, "GET", &format!("{API}/internalusers/admin"), None).0,
        404
    );
}
