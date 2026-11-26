//! Authorization at the route (docs/specs/security-authz/SPEC.md).

use crate::assemble::{assembled_with, config, start_with};
use crate::security_authn::{API, call as call_rig, make_user, rig};
use crate::support;

use oss_node::assemble::{Config, Running};
use oss_node::security_authz::{HANDLER, rule};

const ADMIN: &str = "admin:Adm1n!Password";
const USER: &str = "u:Zz9!longEnough";

fn secured() -> Config {
    Config {
        index_templates: true,
        ingest_pipelines: true,
        knn: true,
        snapshots: true,
        snapshots_take: true,
        security: true,
        security_admin: Some((
            "admin".into(),
            bcrypt::hash("Adm1n!Password", 4).expect("hash"),
        )),
        security_bcrypt_cost: 4,
        plugin_upstreams: vec!["http://127.0.0.1:1".into()],
        ..config()
    }
}

fn send(r: &Running, who: &str, method: &str, target: &str, body: Option<&str>) -> (u16, String) {
    let reply = support::request_as(r.addr(), method, target, body.map(str::as_bytes), Some(who));
    (reply.status, reply.text())
}

/// A node with user `u` holding `role`.
fn node_with(role: &str) -> Running {
    let (r, _) = start_with(secured(), 256 << 20);
    let put = |path: &str, body: &str| {
        let reply = send(&r, ADMIN, "PUT", &format!("{API}/{path}"), Some(body));
        assert!(reply.0 == 200 || reply.0 == 201, "{path}: {reply:?}");
    };
    put("internalusers/u", r#"{"password":"Zz9!longEnough"}"#);
    put("roles/r", role);
    put("rolesmapping/r", r#"{"users":["u"]}"#);
    let made = send(&r, ADMIN, "PUT", "/payroll", None);
    assert_eq!(made.0, 200, "{made:?}");
    r
}

const LOGS_RW: &str = r#"{"cluster_permissions":["indices:data/write/bulk"],
    "index_permissions":[{"index_patterns":["docs*"],"allowed_actions":["read","write"]}]}"#;

// docs/specs/security-authz AC-6
#[test]
fn every_registered_route_has_an_action_row() {
    let config = secured();
    let (node, _, _) = assembled_with(&config, 256 << 20);
    let routes = node.routes();
    assert!(routes.len() > 100, "{}", routes.len());
    let missing: Vec<String> = routes
        .iter()
        .filter(|(m, p)| m.is_some() && rule(*m, p).is_none())
        .map(|(m, p)| format!("{m:?} {p}"))
        .collect();
    assert!(missing.is_empty(), "no row for: {missing:?}");
    assert!(
        routes.iter().any(|(m, _)| m.is_none()),
        "the prefix routes were listed"
    );
}

// docs/specs/security-authz AC-1, AC-4
#[test]
fn a_limited_user_does_what_its_role_grants_and_is_told_what_it_lacks() {
    let r = node_with(LOGS_RW);
    let wrote = send(&r, USER, "PUT", "/docs/_doc/1", Some(r#"{"n":1}"#));
    assert_eq!(wrote.0, 201, "{wrote:?}");
    let denied = send(&r, USER, "PUT", "/payroll/_doc/1", Some(r#"{"n":1}"#));
    assert_eq!(denied.0, 403, "{denied:?}");
    assert!(
        denied.1.contains("no permissions for [indices:data/write/index] and User [name=u, backend_roles=[], requestedTenant=null]"),
        "{denied:?}"
    );
    assert_eq!(send(&r, USER, "DELETE", "/docs", None).0, 403);
    assert_eq!(send(&r, USER, "PUT", "/docs-2", None).0, 403);
    assert_eq!(
        send(&r, USER, "PUT", "/_cluster/settings", Some("{}")).0,
        403
    );
    assert_eq!(
        send(&r, USER, "GET", "/_plugins/_security/authinfo", None).0,
        200
    );
}

// docs/specs/security-authz AC-1, AC-4; docs/specs/node-index-mapping-get AC-7
#[test]
fn reading_a_mapping_needs_the_mappings_get_action() {
    let r = node_with(LOGS_RW);
    assert_eq!(send(&r, ADMIN, "PUT", "/docs", None).0, 200);
    let denied = send(&r, USER, "GET", "/docs/_mapping", None);
    assert_eq!(denied.0, 403, "{denied:?}");
    assert!(
        denied.1.contains("indices:admin/mappings/get"),
        "{denied:?}"
    );
    assert_eq!(send(&r, USER, "GET", "/docs/_mappings", None).0, 403);
    assert_eq!(send(&r, ADMIN, "GET", "/docs/_mapping", None).0, 200);
}

// docs/specs/node-index-mapping-put AC-7
#[test]
fn updating_a_mapping_needs_the_mapping_put_action() {
    // `read` does not grant it; `write` does, as the security plugin's group has it.
    let read_only = r#"{"cluster_permissions":[],
        "index_permissions":[{"index_patterns":["docs*"],"allowed_actions":["read"]}]}"#;
    let r = node_with(read_only);
    assert_eq!(send(&r, ADMIN, "PUT", "/docs", None).0, 200);
    let body = r#"{"properties":{"n":{"type":"integer"}}}"#;
    for method in ["PUT", "POST"] {
        let denied = send(&r, USER, method, "/docs/_mapping", Some(body));
        assert_eq!(denied.0, 403, "{method}: {denied:?}");
        assert!(denied.1.contains("indices:admin/mapping/put"), "{denied:?}");
    }
    assert_eq!(send(&r, ADMIN, "PUT", "/docs/_mapping", Some(body)).0, 200);
    let writer = node_with(LOGS_RW);
    assert_eq!(send(&writer, ADMIN, "PUT", "/docs", None).0, 200);
    assert_eq!(
        send(&writer, USER, "PUT", "/docs/_mapping", Some(body)).0,
        200
    );
}

// docs/specs/security-authz AC-2
#[test]
fn a_comma_list_or_encoded_name_is_checked_element_by_element() {
    let r = node_with(LOGS_RW);
    let search = |target: &str| send(&r, USER, "POST", target, Some("{}")).0;
    assert_ne!(search("/docs/_search"), 403);
    assert_eq!(
        search("/docs,payroll/_search"),
        403,
        "a `*` never spans a comma"
    );
    assert_eq!(
        search("/docs%2Cpayroll/_search"),
        403,
        "an encoded comma is a comma"
    );
    assert!(
        search("/docs%2A/_search") != 403,
        "an encoded wildcard is the wildcard"
    );
    assert_eq!(search("/%2A/_search"), 403);
    assert_eq!(search("/_all/_search"), 403);
    assert_eq!(
        search("/docs,-payroll/_search"),
        403,
        "an exclusion is refused"
    );
    assert_eq!(search("/docs,/_search"), 403, "an empty element is refused");
    assert_eq!(search("/payroll/_search"), 403);
    assert!(
        search("/docs%252Cpayroll/_search") != 200,
        "double encoding is a name, not a list"
    );
}

// docs/specs/security-authz AC-3
#[test]
fn bulk_needs_the_cluster_permission_and_all_access_on_indices() {
    let r = node_with(LOGS_RW);
    let line = "{\"index\":{\"_index\":\"payroll\",\"_id\":\"1\"}}\n{\"n\":1}\n";
    assert_eq!(
        send(&r, USER, "POST", "/docs/_bulk", Some(line)).0,
        403,
        "the path index is only a default"
    );
    assert_eq!(send(&r, USER, "POST", "/_bulk", Some(line)).0, 403);
    let no_cluster = node_with(
        r#"{"index_permissions":[{"index_patterns":["*"],"allowed_actions":["write"]}]}"#,
    );
    assert_eq!(
        send(&no_cluster, USER, "POST", "/_bulk", Some(line)).0,
        403,
        "no cluster permission"
    );
    let both = node_with(
        r#"{"cluster_permissions":["indices:data/write/bulk"],"index_permissions":[{"index_patterns":["*"],"allowed_actions":["write"]}]}"#,
    );
    assert_ne!(send(&both, USER, "POST", "/_bulk", Some(line)).0, 403);
}

// docs/specs/security-authz AC-10
#[test]
fn routes_whose_indices_are_in_the_body_or_that_are_proxied_need_all_access() {
    let r = node_with(LOGS_RW);
    for (m, t) in [
        ("POST", "/_msearch"),
        ("POST", "/docs/_msearch"),
        ("POST", "/_msearch/template"),
        ("POST", "/docs/_msearch/template"),
        ("POST", "/_search"),
        ("POST", "/docs/_delete_by_query"),
        ("PUT", "/_index_template/t"),
        ("GET", "/_cat/indices"),
        ("GET", "/_snapshot"),
        ("POST", "/_plugins/_ml/models"),
        ("DELETE", "/_plugins/_ml/models/x"),
        ("PUT", "/_plugins/_alerting/monitors"),
        ("GET", "/_opendistro/_ism/policies"),
    ] {
        let reply = send(&r, USER, m, t, Some("{}"));
        assert_eq!(reply.0, 403, "{m} {t}: {reply:?}");
    }
    let admin = send(&r, ADMIN, "GET", "/_cat/indices", None);
    assert_eq!(admin.0, 200, "{admin:?}");
}

// docs/specs/security-authz AC-6: the management API stays the handlers' to refuse.
#[test]
fn every_management_route_refuses_a_user_without_the_administrator_role() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    // Every method registered on a management pattern, not a list written here.
    let (node, _, _) = assembled_with(&secured(), 256 << 20);
    let routes = node.routes();
    let mut sent = 0;
    for (method, pattern) in routes.iter().filter(|(_, p)| HANDLER.contains(p)) {
        let Some(method) = method else { continue };
        let path = pattern.replace("{name}", "x");
        let reply = call_rig(&r, Some(USER), method.as_str(), &path, Some("{}"));
        assert_eq!(reply.0, 403, "{method:?} {path}: {reply:?}");
        assert!(
            reply.1.contains("FORBIDDEN"),
            "{method:?} {path}: {reply:?}"
        );
        sent += 1;
    }
    assert!(sent >= 20, "{sent}");
}

// docs/specs/security-authz AC-3: a bulk may delete and update, so it needs those too.
#[test]
fn a_bulk_needs_every_write_action_its_items_may_name() {
    let r = node_with(
        r#"{"cluster_permissions":["indices:data/write/bulk"],"index_permissions":[{"index_patterns":["*"],"allowed_actions":["indices:data/write/index"]}]}"#,
    );
    let delete = "{\"delete\":{\"_index\":\"payroll\",\"_id\":\"1\"}}\n";
    let reply = send(&r, USER, "POST", "/_bulk", Some(delete));
    assert_eq!(reply.0, 403, "{reply:?}");
    assert!(reply.1.contains("indices:data/write/"), "{reply:?}");
}

// docs/specs/security-authz AC-2: a long list is refused rather than checked name by name.
#[test]
fn an_index_list_longer_than_the_cap_is_refused() {
    let r = node_with(LOGS_RW);
    let many = vec!["docs"; 300].join(",");
    assert_eq!(
        send(&r, USER, "GET", &format!("/{many}/_search"), None).0,
        403
    );
    let few = ["docs"; 3].join(",");
    assert_ne!(
        send(&r, USER, "GET", &format!("/{few}/_search"), None).0,
        403
    );
}

// docs/specs/security-authz AC-2: creating an index grants no alias, so an alias in the body stays refused.
#[test]
fn an_index_created_with_an_alias_is_not_made_for_a_user_who_may_only_create() {
    let r = node_with(
        r#"{"index_permissions":[{"index_patterns":["docs*"],"allowed_actions":["indices:admin/create"]}]}"#,
    );
    let reply = send(
        &r,
        USER,
        "PUT",
        "/docsnew",
        Some(r#"{"aliases":{"payroll_alias":{}}}"#),
    );
    assert!(reply.0 != 200 && reply.0 != 201, "{reply:?}");
}

// docs/specs/extension-abi: uploading a module is an administrator's act.
#[cfg(feature = "extensions")]
#[test]
fn only_an_administrator_may_upload_a_module() {
    let r = node_with(LOGS_RW);
    let target = "/_serverless/extensions/modules";
    let limited = send(&r, USER, "PUT", target, Some("not a module"));
    assert_eq!(limited.0, 403, "{limited:?}");
    let admin = send(&r, ADMIN, "PUT", target, Some("not a module"));
    assert_eq!(
        admin.0, 400,
        "past the check, refused for what it is: {admin:?}"
    );
}

// docs/specs/stock-snapshot-export AC-8: registering a repository is a cluster
// administrator's act, so a user with index permissions is refused and
// nothing is registered.
#[test]
fn only_an_administrator_may_register_or_delete_a_repository() {
    let r = node_with(LOGS_RW);
    let body = r#"{"type":"fs","settings":{"location":"/tmp/oss-authz-repo"}}"#;
    for (m, t) in [
        ("PUT", "/_snapshot/r1"),
        ("POST", "/_snapshot/r1"),
        ("DELETE", "/_snapshot/r1"),
        ("POST", "/_snapshot/r1/_verify"),
    ] {
        let reply = send(&r, USER, m, t, Some(body));
        assert_eq!(reply.0, 403, "{m} {t}: {reply:?}");
    }
    let listed = send(&r, ADMIN, "GET", "/_snapshot/r1", None);
    assert_eq!(
        listed.0, 404,
        "the refused PUT registered nothing: {listed:?}"
    );
    let admin = send(&r, ADMIN, "PUT", "/_snapshot/r1", Some(body));
    assert_ne!(admin.0, 403, "past the check: {admin:?}");
}

// docs/specs/extension-analyzer AC-7, AC-9: a pin, in any spelling of `index.analysis`, needs
// `all_access` in the create's handler, whatever the role grants on the index.
#[cfg(feature = "extensions")]
#[test]
fn a_pin_in_any_spelling_needs_all_access_to_create() {
    let r = node_with(
        r#"{"index_permissions":[{"index_patterns":["pins*"],"allowed_actions":["indices:admin/create"]}]}"#,
    );
    let allowed = send(
        &r,
        ADMIN,
        "PUT",
        "/_cluster/settings",
        Some(r#"{"persistent":{"serverless.extensions.pins.allowed":true}}"#),
    );
    assert_eq!(allowed.0, 200, "{allowed:?}");
    let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let pin = format!(r#""type":"serverless_module","module":"{hash}","version":"1""#);
    let spellings = [
        format!(
            r#"{{"settings":{{"index.analysis.analyzer.ext_a.type":"serverless_module","index.analysis.analyzer.ext_a.module":"{hash}","index.analysis.analyzer.ext_a.version":"1"}}}}"#
        ),
        format!(
            r#"{{"settings":{{"index":{{"analysis":{{"analyzer":{{"ext_a":{{{pin}}}}}}}}}}}}}"#
        ),
        format!(r#"{{"settings":{{"analysis":{{"analyzer":{{"ext_a":{{{pin}}}}}}}}}}}"#),
    ];
    for (i, body) in spellings.iter().enumerate() {
        let name = format!("pins{i}");
        let limited = send(&r, USER, "PUT", &format!("/{name}"), Some(body));
        assert_eq!(limited.0, 403, "spelling {i}: {limited:?}");
        // The administrator is past the check, and refused for the module the store lacks.
        let admin = send(&r, ADMIN, "PUT", &format!("/{name}"), Some(body));
        assert!(
            admin.0 == 400 && admin.1.contains("not in the store"),
            "spelling {i}: {admin:?}"
        );
    }
    // The same user creates an index with no pin.
    let plain = send(&r, USER, "PUT", "/pinsplain", Some("{}"));
    assert_eq!(plain.0, 200, "{plain:?}");
}
