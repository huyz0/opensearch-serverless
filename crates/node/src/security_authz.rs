//! What each route needs of a user, and the authorizer that asks
//! ([ADR 0051](../../../docs/adr/0051-authorization-at-the-route-by-action-table.md)).

use std::sync::Arc;

use oss_coordinator::security::{Config, forbidden};
use oss_rest::{Authorization, Authorizer, Denial, Method};

use crate::security_route::admin_refusal;
use crate::security_state::SecurityState;

const JSON: &str = "application/json; charset=UTF-8";
const TEXT: &str = "text/plain; charset=UTF-8";

/// What a route needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Any authenticated user.
    Open,
    /// A cluster-level action.
    Cluster(&'static str),
    /// An action on the indices named by the path's `{index}`.
    Index(&'static str),
    /// A `_bulk`: the cluster permission `indices:data/write/bulk` and
    /// write on every index, since its items name their own.
    Bulk,
    /// Decided by the handler, which answers the recorded `FORBIDDEN` of the
    /// management API to a user without the administrator role.
    Handler,
    /// What `all_access` holds: the route's indices are not in its path, it
    /// lists or changes more than one index, or it is proxied.
    AllAccess,
}

const GET: &[Method] = &[Method::Get];
const GET_POST: &[Method] = &[Method::Get, Method::Post];
const GET_HEAD: &[Method] = &[Method::Get, Method::Head];
const WRITES: &[Method] = &[Method::Put, Method::Post];

/// The rules: methods, pattern, rule. A pattern in [`ALL_ACCESS`] is
/// `all_access` for every method.
const RULES: &[(&[Method], &str, Rule)] = &[
    (GET, "/_plugins/_security/authinfo", Rule::Open),
    (
        &[Method::Get],
        "/_cluster/health",
        Rule::Cluster("cluster:monitor/health"),
    ),
    (
        &[Method::Get],
        "/_cluster/settings",
        Rule::Cluster("cluster:monitor/state"),
    ),
    (
        &[Method::Put],
        "/_cluster/settings",
        Rule::Cluster("cluster:admin/settings/update"),
    ),
    (WRITES, "/_bulk", Rule::Bulk),
    (WRITES, "/{index}/_bulk", Rule::Bulk),
    (
        GET_POST,
        "/{index}/_search",
        Rule::Index("indices:data/read/search"),
    ),
    (
        GET_POST,
        "/{index}/_count",
        Rule::Index("indices:data/read/search"),
    ),
    (
        GET_HEAD,
        "/{index}/_doc/{id}",
        Rule::Index("indices:data/read/get"),
    ),
    (
        GET_HEAD,
        "/{index}/_source/{id}",
        Rule::Index("indices:data/read/get"),
    ),
    (
        &[Method::Post],
        "/{index}/_doc",
        Rule::Index("indices:data/write/index"),
    ),
    (
        WRITES,
        "/{index}/_doc/{id}",
        Rule::Index("indices:data/write/index"),
    ),
    (
        WRITES,
        "/{index}/_create/{id}",
        Rule::Index("indices:data/write/index"),
    ),
    (
        &[Method::Delete],
        "/{index}/_doc/{id}",
        Rule::Index("indices:data/write/delete"),
    ),
    (
        &[Method::Put],
        "/{index}",
        Rule::Index("indices:admin/create"),
    ),
    (
        &[Method::Delete],
        "/{index}",
        Rule::Index("indices:admin/delete"),
    ),
    (
        &[Method::Head],
        "/{index}",
        Rule::Index("indices:admin/exists"),
    ),
    (
        GET_POST,
        "/{index}/_refresh",
        Rule::Index("indices:admin/refresh"),
    ),
    (
        GET_POST,
        "/{index}/_field_caps",
        Rule::Index("indices:data/read/field_caps"),
    ),
    (
        GET_POST,
        "/{index}/_validate/query",
        Rule::Index("indices:admin/validate/query"),
    ),
    (
        GET_POST,
        "/{index}/_analyze",
        Rule::Index("indices:admin/analyze"),
    ),
    (
        GET_POST,
        "/{index}/_rank_eval",
        Rule::Index("indices:data/read/rank_eval"),
    ),
    (
        GET_POST,
        "/{index}/_search/template",
        Rule::Index("indices:data/read/search/template"),
    ),
    (
        &[Method::Post],
        "/{index}/_pit",
        Rule::Index("indices:data/read/point_in_time/create"),
    ),
    (
        &[Method::Post],
        "/{index}/_search/point_in_time",
        Rule::Index("indices:data/read/point_in_time/create"),
    ),
];

/// The management API's patterns, for every method: the handler decides.
pub const HANDLER: &[&str] = &[
    "/_plugins/_security/api/actiongroups",
    "/_plugins/_security/api/actiongroups/{name}",
    "/_plugins/_security/api/internalusers",
    "/_plugins/_security/api/internalusers/{name}",
    "/_plugins/_security/api/roles",
    "/_plugins/_security/api/roles/{name}",
    "/_plugins/_security/api/rolesmapping",
    "/_plugins/_security/api/rolesmapping/{name}",
    "/_plugins/_security/api/tenants",
    "/_plugins/_security/api/tenants/{name}",
];

/// Routes that are `all_access` for every method, each named so that a
/// route added without a row is seen. See the ADR for why each is here.
const ALL_ACCESS: &[&str] = &[
    "/_analyze",
    "/_field_caps",
    "/_msearch",
    "/_search",
    "/_rank_eval",
    "/_render/template",
    "/_render/template/{id}",
    "/_search/template",
    "/_validate/query",
    "/{index}/_msearch",
    "/{index}/_delete_by_query",
    "/_cat",
    "/_cat/health",
    "/_cat/indices",
    "/_cat/indices/{index}",
    "/_list/indices",
    "/_list/indices/{index}",
    "/_cluster/health/{index}",
    "/_component_template",
    "/_component_template/{name}",
    "/_index_template",
    "/_index_template/{name}",
    "/_ingest/pipeline",
    "/_ingest/pipeline/{id}",
    "/_snapshot",
    "/_snapshot/{repo}",
    "/_snapshot/{repo}/_verify",
    "/_snapshot/{repo}/{name}/_restore",
    "/_snapshot/{repo}/{name}",
    "/_search/point_in_time",
    "/_search/point_in_time/_all",
    "/_search/point_in_time/{id}",
    "/_pit/{id}",
    "/_serverless/usage",
];

/// The rule for a registered route, or none when the table has no row for
/// it. A prefix route (`method` none) is [`Rule::AllAccess`].
pub fn rule(method: Option<Method>, pattern: &str) -> Option<Rule> {
    let Some(method) = method else {
        return Some(Rule::AllAccess);
    };
    if HANDLER.contains(&pattern) {
        return Some(Rule::Handler);
    }
    if ALL_ACCESS.contains(&pattern) {
        return Some(Rule::AllAccess);
    }
    RULES
        .iter()
        .find(|(methods, p, _)| *p == pattern && methods.contains(&method))
        .map(|(_, _, r)| *r)
}

/// The rule a request is held to: its row, and `all_access` for a prefix
/// route or a route with no row.
pub fn rule_of(prefix: bool, method: Option<Method>, pattern: &str) -> Rule {
    if prefix {
        return Rule::AllAccess;
    }
    rule(method, pattern).unwrap_or(Rule::AllAccess)
}

/// The most names one index expression may list; a longer one is refused.
const MAX_ELEMENTS: usize = 256;

/// Splits an index expression the way the handler will, refusing what a
/// handler would read differently: an empty element, an exclusion, an
/// inclusion sign. `_all` is `*`.
fn elements(expression: &str) -> Option<Vec<&str>> {
    // ALLOC: one request's index names, at most the path's length.
    let mut out = Vec::new();
    for e in expression.split(',') {
        if out.len() >= MAX_ELEMENTS || e.is_empty() || e.starts_with(['-', '+']) {
            return None;
        }
        out.push(if e == "_all" { "*" } else { e });
    }
    Some(out)
}

/// The authorizer a node with security on puts on its router.
pub struct RouteAuthorizer {
    state: Arc<SecurityState>,
}

impl RouteAuthorizer {
    /// An authorizer over `state`.
    pub fn new(state: Arc<SecurityState>) -> Self {
        Self { state }
    }
}

fn refuse(config: &Config, user: &str, action: &str) -> Denial {
    Denial {
        status: 403,
        content_type: JSON,
        body: forbidden(config, user, action),
        challenge: None,
    }
}

fn allowed(config: &Config, user: &str, rule: Rule, req: &Authorization<'_>) -> Result<(), String> {
    match rule {
        Rule::Open | Rule::Handler => Ok(()),
        Rule::Cluster(action) => config
            .cluster_allows(user, action)
            .then_some(())
            .ok_or_else(|| action.to_owned()),
        Rule::Bulk => {
            let action = "indices:data/write/bulk";
            if !config.cluster_allows(user, action) {
                return Err(action.to_owned());
            }
            // A bulk body may carry any of these, so each is needed.
            ["index", "create", "update", "delete"]
                .iter()
                .map(|kind| format!("indices:data/write/{kind}"))
                .find(|item| !config.indices_allow(user, item, &["*"]))
                .map_or(Ok(()), Err)
        }
        Rule::Index(action) => {
            let names = match req.path_param("index") {
                Some(Some(expression)) => elements(&expression)
                    // ALLOC: one request's index names, at most the path's length.
                    .map(|v| v.into_iter().map(str::to_owned).collect::<Vec<_>>()),
                // A route with no `{index}` or an undecodable one is not allowed by a name.
                _ => None,
            };
            let Some(names) = names else {
                return Err(action.to_owned());
            };
            // ALLOC: one request's index names, at most the path's length.
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            config
                .indices_allow(user, action, &refs)
                .then_some(())
                .ok_or_else(|| action.to_owned())
        }
        Rule::AllAccess => config.all_access(user).then_some(()).ok_or_else(|| {
            // ALLOC: one refusal, once.
            format!(
                "rest:{} {}",
                req.method().map_or("?", |m| m.as_str()),
                req.pattern()
            )
        }),
    }
}

impl Authorizer for RouteAuthorizer {
    fn authorize(&self, req: &Authorization<'_>) -> Option<Denial> {
        let Ok(config) = self.state.config() else {
            return Some(Denial {
                status: 503,
                content_type: TEXT,
                body: "The security configuration is unavailable".to_owned(),
                challenge: None,
            });
        };
        let Some(user) = req.principal() else {
            return Some(Denial {
                status: 403,
                content_type: JSON,
                body: forbidden(&config, "", "rest:unauthenticated"),
                challenge: None,
            });
        };
        let rule = rule_of(req.is_prefix(), req.method(), req.pattern());
        if rule == Rule::Handler {
            // The management API answers the plugin's own refusal body; the
            // handler checks again, so a route never rests on one check.
            return admin_refusal(&config, user).map(|a| Denial {
                status: a.status,
                content_type: JSON,
                body: a.body,
                challenge: None,
            });
        }
        allowed(&config, user, rule, req)
            .err()
            .map(|action| refuse(&config, user, &action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // docs/specs/security-authz AC-6
    #[test]
    fn a_route_with_no_row_and_a_prefix_route_need_all_access() {
        assert_eq!(rule(Some(Method::Get), "/new/{index}/route"), None);
        assert_eq!(
            rule_of(false, Some(Method::Get), "/new/{index}/route"),
            Rule::AllAccess
        );
        assert_eq!(rule_of(true, None, "/_plugins/_ml"), Rule::AllAccess);
        assert_eq!(
            rule_of(false, Some(Method::Get), "/{index}/_search"),
            Rule::Index("indices:data/read/search")
        );
        assert_eq!(
            rule_of(false, Some(Method::Delete), "/{index}/_search"),
            Rule::AllAccess,
            "a method with no row for a known pattern is not allowed through"
        );
    }
}
