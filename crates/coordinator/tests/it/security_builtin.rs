//! The documents the plugin ships (docs/specs/security-complete/SPEC.md, slice 2).

use oss_coordinator::security::*;
use serde_json::Value;

const RECORDING: &str = include_str!("../../../../fixtures/compat/security-api/security-api.json");

fn recorded(step: &str) -> Value {
    let all: Value = serde_json::from_str(RECORDING).expect("json");
    all["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|s| s["name"] == step)
        .expect("step")["response"]["body"]
        .clone()
}

#[test]
fn the_built_in_set_is_the_recorded_one() {
    for (kind, step) in [
        (Kind::Roles, "list roles"),
        (Kind::ActionGroups, "list actiongroups"),
        (Kind::Tenants, "list tenants"),
    ] {
        let list = recorded(step);
        let reserved: Vec<(&String, &Value)> = list
            .as_object()
            .expect("object")
            .iter()
            .filter(|(_, v)| v["reserved"] == true)
            .collect();
        assert_eq!(builtins(kind).len(), reserved.len(), "{kind:?}");
        for (name, entry) in reserved {
            let shipped: Value =
                serde_json::from_str(&read_builtin(kind, name).expect("shipped")).expect("json");
            assert_eq!(&shipped[name.as_str()], entry, "{kind:?} {name}");
        }
    }
}

#[test]
fn no_user_or_mapping_is_shipped() {
    assert!(builtins(Kind::InternalUsers).is_empty());
    assert!(builtins(Kind::RolesMapping).is_empty());
    assert!(builtin(Kind::Tenants, "admin_tenant").is_none());
}

fn refusal(kind: Kind, name: &str) -> Option<(u16, String)> {
    protection(kind, name).map(|a| {
        let body: Value = serde_json::from_str(&a.body).expect("json");
        (
            a.status,
            body["message"].as_str().expect("message").to_owned(),
        )
    })
}

#[test]
fn a_change_to_a_shipped_document_is_refused_as_the_plugin_refuses_it() {
    assert_eq!(
        refusal(Kind::Roles, "kibana_user"),
        Some((403, "Resource 'kibana_user' is static.".to_owned()))
    );
    assert_eq!(
        refusal(Kind::ActionGroups, "read"),
        Some((403, "Resource 'read' is static.".to_owned()))
    );
    assert_eq!(
        refusal(Kind::Tenants, "global_tenant"),
        Some((403, "Resource 'global_tenant' is static.".to_owned()))
    );
    assert_eq!(
        refusal(Kind::Roles, "security_rest_api_access"),
        Some((
            403,
            "Resource 'security_rest_api_access' is reserved.".to_owned()
        ))
    );
    assert_eq!(refusal(Kind::Roles, "mine"), None);
    assert_eq!(refusal(Kind::InternalUsers, "kibana_user"), None);
}

#[test]
fn a_reserved_role_grants_what_the_plugin_grants() {
    let authz = Authz::new();
    let shipped: Value =
        serde_json::from_str(&read_builtin(Kind::Roles, "alerting_read_access").expect("role"))
            .expect("json");
    let cluster = shipped["alerting_read_access"]["cluster_permissions"][0]
        .as_str()
        .expect("a permission")
        .to_owned();
    let role = vec!["alerting_read_access".to_owned()];
    assert!(authz.cluster_allows(&role, &cluster.replace('*', "x")));
    assert!(!authz.cluster_allows(&role, "cluster:admin/settings/update"));
}
