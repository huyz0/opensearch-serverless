//! The answers of the security plugin's configuration and information
//! routes: `securityconfig`, `permissionsinfo`, the routes stock keeps
//! closed, and the cache flush.
//!
//! Shapes are the recorded 3.10.0 answers
//! (`fixtures/compat/security-api/security-api.json`).

use oss_doc::mapping::json::write_str;

use super::{Answer, Config, Rules, Who};

/// The configuration this node enforces, in the plugin's shape: HTTP basic
/// against the internal users is the one authentication domain, nothing
/// authorizes beyond the roles, and Dashboards tenancy is off. Slices that
/// add an authentication backend extend this answer with it.
const SECURITY_CONFIG: &str = r#"{"config":{"dynamic":{"api_tokens":{"enabled":false,"max_duration_seconds":7776000,"max_tokens":100},"auth_failure_listeners":{},"authc":{"basic_internal_auth_domain":{"authentication_backend":{"config":{},"type":"intern"},"description":"Authenticate via HTTP Basic against internal users database","http_authenticator":{"challenge":true,"config":{},"type":"basic"},"http_enabled":true,"order":4}},"authz":{},"disable_intertransport_auth":false,"disable_rest_auth":false,"do_not_fail_on_forbidden":false,"do_not_fail_on_forbidden_empty":false,"filtered_alias_mode":"warn","hosts_resolver_mode":"ip-only","http":{"anonymous_auth_enabled":false,"xff":{"enabled":false,"internalProxies":"192\\.168\\.0\\.10|192\\.168\\.0\\.11","remoteIpHeader":"X-Forwarded-For"}},"kibana":{"default_tenant":"","index":".kibana","multitenancy_enabled":false,"preferred_tenants":[],"private_tenant_enabled":false,"server_username":"kibanaserver"},"multi_rolespan_enabled":true,"on_behalf_of":{"enabled":false},"privileges_evaluation_ignore_unauthorized_indices":true,"respect_request_indices_options":false}}}"#;

/// `GET /_plugins/_security/api/securityconfig`. With a JSON Web Token
/// domain configured it lists `jwt_auth_domain` beside the basic one, its
/// signing key redacted.
pub fn securityconfig(jwt: Option<&Rules>) -> String {
    let Some(rules) = jwt else {
        // ALLOC: one answer a request.
        return SECURITY_CONFIG.to_owned();
    };
    // ALLOC: one answer a request.
    let mut config = String::new();
    let mut roles = String::from("null");
    if let Some(key) = &rules.roles_key {
        roles.clear();
        write_str(&mut roles, key);
    }
    // ALLOC: one answer a request.
    let mut subject = String::new();
    write_str(&mut subject, &rules.subject_key);
    // ALLOC: one answer a request.
    let domain = format!(
        r#","jwt_auth_domain":{{"authentication_backend":{{"config":{{}},"type":"noop"}},"description":"Authenticate via a JSON Web Token","http_authenticator":{{"challenge":false,"config":{{"jwt_header":"Authorization","roles_key":{roles},"signing_key":"******","subject_key":{subject}}},"type":"jwt"}},"http_enabled":true,"order":1}}"#
    );
    // The basic domain is the last of `authc`: its closing brace ends it.
    let marker = r#""order":4}"#;
    match SECURITY_CONFIG.split_once(marker) {
        Some((before, after)) => {
            config.push_str(before);
            config.push_str(marker);
            config.push_str(&domain);
            config.push_str(after);
        }
        None => config.push_str(SECURITY_CONFIG),
    }
    config
}

/// What stock answers for the routes it keeps closed unless its
/// `restapi.allow_securityconfig_modification` is set: changing
/// `securityconfig`, the allow list, and the TLS certificates.
pub fn access_denied() -> Answer {
    Answer {
        status: 403,
        // ALLOC: one answer a request.
        body: r#"{"status":"FORBIDDEN","message":"Access denied"}"#.to_owned(),
    }
}

/// `DELETE /_plugins/_security/api/cache`.
pub fn cache_flushed() -> Answer {
    Answer {
        status: 200,
        // ALLOC: one answer a request.
        body: r#"{"status":"OK","message":"Cache flushed successfully."}"#.to_owned(),
    }
}

/// `GET /_plugins/_security/api/permissionsinfo` for an administrator.
pub fn permissionsinfo<'a>(config: &'a Config, who: impl Into<Who<'a>>) -> String {
    let who = who.into();
    let name = who.name;
    let backend = config.backend_roles_of(who).join(", ");
    // ALLOC: one answer a request.
    let mut who = String::new();
    // ALLOC: one answer a request.
    write_str(
        &mut who,
        &format!("User [name={name}, backend_roles=[{backend}], requestedTenant=null]"),
    );
    // ALLOC: one answer a request.
    let mut user = String::new();
    write_str(&mut user, name);
    // ALLOC: one answer a request.
    format!(
        r#"{{"user":{who},"user_name":{user},"has_api_access":true,"disabled_endpoints":{{}}}}"#
    )
}
