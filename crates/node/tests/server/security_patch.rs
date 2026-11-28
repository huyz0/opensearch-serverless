//! List and `PATCH` of the security configuration
//! (docs/specs/security-complete/SPEC.md, slice 1).

use crate::security_route::{API, Rig, call, rig, stored, total};

use oss_blob::{BlobStore, S3Op};
use oss_format::DocKind;

fn put_tenants(r: &Rig, names: &[&str]) {
    for n in names {
        let put = call(
            r,
            "PUT",
            &format!("{API}/tenants/{n}"),
            Some(&format!(r#"{{"description":"d-{n}"}}"#)),
        );
        assert_eq!(put.0, 201, "{}", put.1);
    }
}

#[test]
fn an_empty_kind_lists_as_an_empty_object() {
    let r = rig();
    assert_eq!(
        call(&r, "GET", &format!("{API}/internalusers"), None),
        (200, "{}".to_owned())
    );
}

#[test]
fn a_list_holds_every_document_as_a_read_renders_it() {
    let r = rig();
    put_tenants(&r, &["b", "a"]);
    call(
        &r,
        "PUT",
        &format!("{API}/internalusers/u"),
        Some(r#"{"password":"Zz9!longEnough"}"#),
    );
    let listed = call(&r, "GET", &format!("{API}/tenants"), None);
    assert_eq!(listed.0, 200, "{}", listed.1);
    let listed: serde_json::Value = serde_json::from_str(&listed.1).expect("json");
    assert_eq!(
        listed
            .as_object()
            .expect("object")
            .keys()
            .collect::<Vec<_>>(),
        ["a", "b", "global_tenant"]
    );
    for n in ["a", "b"] {
        let one = call(&r, "GET", &format!("{API}/tenants/{n}"), None);
        let one: serde_json::Value = serde_json::from_str(&one.1).expect("json");
        assert_eq!(listed[n], one[n]);
    }
    let users = call(&r, "GET", &format!("{API}/internalusers"), None);
    assert_eq!(users.0, 200);
    assert!(
        !users.1.contains("$2") && !users.1.contains("Zz9!"),
        "{}",
        users.1
    );
}

#[test]
fn a_list_costs_a_marker_and_a_get_a_document() {
    let r = rig();
    put_tenants(&r, &["a", "b", "c"]);
    let gets = [S3Op::Get, S3Op::GetRange, S3Op::Head];
    let (g0, p0) = (total(&r, &gets), total(&r, &[S3Op::Put]));
    assert_eq!(call(&r, "GET", &format!("{API}/tenants"), None).0, 200);
    assert_eq!(
        (total(&r, &gets) - g0, total(&r, &[S3Op::Put]) - p0),
        (4, 0)
    );
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}

#[test]
fn a_list_over_the_bound_is_refused_whole() {
    let r = rig();
    // 70 documents of about 60 KiB (the rig's buffer is 64 KiB) pass the 4 MiB bound.
    let big = format!(r#"{{"description":"{}"}}"#, "x".repeat(60_000));
    for i in 0..70 {
        let put = call(&r, "PUT", &format!("{API}/tenants/t{i}"), Some(&big));
        assert_eq!(put.0, 201, "{i}: {:.200}", put.1);
    }
    let over = call(&r, "GET", &format!("{API}/tenants"), None);
    assert_eq!(over.0, 413, "{:.200}", over.1);
    assert!(
        over.1.contains("security_list_too_large"),
        "{:.200}",
        over.1
    );
    assert_eq!(call(&r, "GET", &format!("{API}/tenants/t0"), None).0, 200);
}

#[test]
fn patch_applies_each_operation() {
    let r = rig();
    call(
        &r,
        "PUT",
        &format!("{API}/roles/p"),
        Some(r#"{"cluster_permissions":["a"],"description":"d"}"#),
    );
    let done = call(
        &r,
        "PATCH",
        &format!("{API}/roles/p"),
        Some(
            r#"[{"op":"add","path":"/cluster_permissions/-","value":"b"},
                {"op":"replace","path":"/description","value":"e"}]"#,
        ),
    );
    assert_eq!(
        done,
        (
            200,
            r#"{"status":"OK","message":"'p' updated."}"#.to_owned()
        )
    );
    let stored = stored(&r, DocKind::SecurityRoles, "p");
    let stored: serde_json::Value = serde_json::from_str(&stored).expect("json");
    assert_eq!(
        stored,
        serde_json::json!({"cluster_permissions":["a","b"],"description":"e"})
    );
    let gone = call(
        &r,
        "PATCH",
        &format!("{API}/roles/p"),
        Some(r#"[{"op":"remove","path":"/description"}]"#),
    );
    assert_eq!(gone.0, 200, "{}", gone.1);
    assert!(!self::stored(&r, DocKind::SecurityRoles, "p").contains("description"));
}

#[test]
fn a_patch_that_makes_an_invalid_document_changes_nothing() {
    let r = rig();
    put_tenants(&r, &["t"]);
    let before = stored(&r, DocKind::SecurityTenants, "t");
    for ops in [
        r#"[{"op":"add","path":"/bogus","value":1}]"#,
        r#"[{"op":"replace","path":"/description","value":5}]"#,
        r#"[{"op":"remove","path":"/nope"}]"#,
        r#"[{"op":"remove","path":""}]"#,
    ] {
        let refused = call(&r, "PATCH", &format!("{API}/tenants/t"), Some(ops));
        assert_eq!(refused.0, 400, "{ops}: {}", refused.1);
    }
    assert_eq!(stored(&r, DocKind::SecurityTenants, "t"), before);
}

#[test]
fn a_patch_of_the_wrong_shape_is_400() {
    let r = rig();
    put_tenants(&r, &["t"]);
    for body in [
        None,
        Some("{}"),
        Some("not json"),
        Some(r#"[{"op":"move"}]"#),
    ] {
        let refused = call(&r, "PATCH", &format!("{API}/tenants/t"), body);
        assert_eq!(refused.0, 400, "{body:?}: {}", refused.1);
    }
}

#[test]
fn a_patch_of_an_absent_document_is_404() {
    let r = rig();
    let (g0, p0) = (total(&r, &[S3Op::Put]), 0);
    let refused = call(
        &r,
        "PATCH",
        &format!("{API}/tenants/ghost"),
        Some(r#"[{"op":"add","path":"/description","value":"x"}]"#),
    );
    assert_eq!(refused.0, 404, "{}", refused.1);
    assert_eq!(
        (total(&r, &[S3Op::Put]) - g0, p0),
        (0, 0),
        "nothing written"
    );
}

#[test]
fn a_patched_password_is_stored_as_a_hash() {
    let r = rig();
    call(
        &r,
        "PUT",
        &format!("{API}/internalusers/u"),
        Some(r#"{"password":"Zz9!longEnough"}"#),
    );
    let old = stored(&r, DocKind::SecurityInternalUsers, "u");
    let patched = call(
        &r,
        "PATCH",
        &format!("{API}/internalusers/u"),
        Some(r#"[{"op":"add","path":"/password","value":"Another!pass1"}]"#),
    );
    assert_eq!(patched.0, 200, "{}", patched.1);
    let now = stored(&r, DocKind::SecurityInternalUsers, "u");
    assert!(
        !now.contains("Another!pass1") && !now.contains("password"),
        "{now}"
    );
    assert_ne!(now, old);
    let hash = now
        .split(r#""hash":""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("a hash");
    assert!(bcrypt::verify("Another!pass1", hash).expect("verifies"));
    let weak = call(
        &r,
        "PATCH",
        &format!("{API}/internalusers/u"),
        Some(r#"[{"op":"add","path":"/password","value":"short"}]"#),
    );
    assert_eq!(weak.0, 400, "{}", weak.1);
}

#[test]
fn a_patch_costs_what_a_replacing_put_costs() {
    let r = rig();
    put_tenants(&r, &["t"]);
    let gets = [S3Op::Get, S3Op::GetRange, S3Op::Head];
    let (g0, p0) = (total(&r, &gets), total(&r, &[S3Op::Put]));
    let done = call(
        &r,
        "PATCH",
        &format!("{API}/tenants/t"),
        Some(r#"[{"op":"replace","path":"/description","value":"y"}]"#),
    );
    assert_eq!(done.0, 200, "{}", done.1);
    assert_eq!(
        (total(&r, &gets) - g0, total(&r, &[S3Op::Put]) - p0),
        (3, 2)
    );
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}

#[test]
fn a_bulk_patch_changes_several_documents() {
    let r = rig();
    put_tenants(&r, &["keep", "edit", "drop"]);
    let done = call(
        &r,
        "PATCH",
        &format!("{API}/tenants"),
        Some(
            r#"[{"op":"add","path":"/fresh","value":{"description":"new"}},
                {"op":"replace","path":"/edit/description","value":"changed"},
                {"op":"remove","path":"/drop"}]"#,
        ),
    );
    assert_eq!(
        done,
        (
            200,
            r#"{"status":"OK","message":"Resource updated."}"#.to_owned()
        )
    );
    let listed: serde_json::Value =
        serde_json::from_str(&call(&r, "GET", &format!("{API}/tenants"), None).1).expect("json");
    let names: Vec<&String> = listed.as_object().expect("object").keys().collect();
    assert_eq!(names, ["edit", "fresh", "global_tenant", "keep"]);
    assert_eq!(listed["edit"]["description"], "changed");
    assert_eq!(listed["fresh"]["description"], "new");
}

#[test]
fn an_invalid_operation_in_a_bulk_patch_writes_nothing() {
    let r = rig();
    put_tenants(&r, &["a"]);
    let before = total(&r, &[S3Op::Put]);
    for ops in [
        r#"[{"op":"replace","path":"/a/description","value":"x"},{"op":"add","path":"/b","value":{"bogus":1}}]"#,
        r#"[{"op":"replace","path":"/a/description","value":"x"},{"op":"replace","path":"/ghost/description","value":"x"}]"#,
    ] {
        let refused = call(&r, "PATCH", &format!("{API}/tenants"), Some(ops));
        assert!(refused.0 == 400 || refused.0 == 404, "{ops}: {}", refused.1);
    }
    assert_eq!(total(&r, &[S3Op::Put]), before, "no write");
    assert!(
        call(&r, "GET", &format!("{API}/tenants/a"), None)
            .1
            .contains("d-a")
    );
    // A mapping for a role that is not there is refused like the put.
    let mapped = call(
        &r,
        "PATCH",
        &format!("{API}/rolesmapping"),
        Some(r#"[{"op":"add","path":"/nope","value":{"users":["u"]}}]"#),
    );
    assert_eq!(mapped.0, 404, "{}", mapped.1);
}

#[test]
fn a_bulk_patch_over_100_documents_is_refused() {
    let r = rig();
    let ops: Vec<String> = (0..101)
        .map(|i| format!(r#"{{"op":"add","path":"/t{i}","value":{{}}}}"#))
        .collect();
    let refused = call(
        &r,
        "PATCH",
        &format!("{API}/tenants"),
        Some(&format!("[{}]", ops.join(","))),
    );
    assert_eq!(refused.0, 400, "{}", refused.1);
    let after = call(&r, "GET", &format!("{API}/tenants"), None).1;
    assert_eq!(
        after.matches("\"description\"").count(),
        1,
        "only the shipped tenant: {after}"
    );
}

#[test]
fn removing_what_is_not_there_changes_nothing() {
    let r = rig();
    put_tenants(&r, &["a"]);
    let before = total(&r, &[S3Op::Put]);
    let done = call(
        &r,
        "PATCH",
        &format!("{API}/tenants"),
        Some(r#"[{"op":"remove","path":"/ghost"}]"#),
    );
    assert_eq!(
        done,
        (
            200,
            r#"{"status":"OK","message":"No updates required"}"#.to_owned()
        )
    );
    assert_eq!(total(&r, &[S3Op::Put]), before);
}
