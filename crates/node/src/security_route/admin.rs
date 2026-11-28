//! `securityconfig`, `permissionsinfo`, the routes stock keeps closed, and
//! the cache flush (docs/specs/security-complete/SPEC.md, slice 3).

use std::sync::Arc;

use oss_coordinator::security::{access_denied, cache_flushed, permissionsinfo, securityconfig};
use oss_rest::{JSON, Method, Request, Response, RouteError, Router};

use super::SecurityService;
use crate::route_util::refuse;
use crate::security_state::SecurityState;

/// The configuration and information routes, for administrators only.
///
/// # Errors
///
/// A route registered twice.
pub fn config_routes(
    router: Router,
    service: &Arc<SecurityService>,
    state: &Arc<SecurityState>,
) -> Result<Router, RouteError> {
    let mut router = router;
    // Stock answers 403 to these unless it is set to allow their change.
    for (method, path) in [
        (Method::Put, "/_plugins/_security/api/securityconfig/config"),
        (Method::Patch, "/_plugins/_security/api/securityconfig"),
        (Method::Get, "/_plugins/_security/api/allowlist"),
        (Method::Put, "/_plugins/_security/api/allowlist"),
        (Method::Get, "/_plugins/_security/api/ssl/certs"),
    ] {
        let service = Arc::clone(service);
        router = router.route(
            method,
            path,
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                let refused = match service.admin_only(req)? {
                    Some(refused) => refused,
                    None => access_denied(),
                };
                resp.send(refused.status, JSON, refused.body.as_bytes())
            },
        )?;
    }
    let read = Arc::clone(service);
    router = router.route(
        Method::Get,
        "/_plugins/_security/api/securityconfig",
        &[],
        move |req: &mut Request<'_>, resp: &mut Response<'_>| {
            if let Some(refused) = read.admin_only(req)? {
                return resp.send(refused.status, JSON, refused.body.as_bytes());
            }
            resp.send(200, JSON, securityconfig(read.jwt_rules()).as_bytes())
        },
    )?;
    let (info, held) = (Arc::clone(service), Arc::clone(state));
    router = router.route(
        Method::Get,
        "/_plugins/_security/api/permissionsinfo",
        &[],
        move |req: &mut Request<'_>, resp: &mut Response<'_>| {
            if let Some(refused) = info.admin_only(req)? {
                return resp.send(refused.status, JSON, refused.body.as_bytes());
            }
            let config = held.config().map_err(|_| {
                refuse(
                    503,
                    "security_exception",
                    "the security configuration is unavailable",
                )
            })?;
            let body = permissionsinfo(&config, super::who_of(req));
            resp.send(200, JSON, body.as_bytes())
        },
    )?;
    let (flush, held) = (Arc::clone(service), Arc::clone(state));
    router.route(
        Method::Delete,
        "/_plugins/_security/api/cache",
        &[],
        move |req: &mut Request<'_>, resp: &mut Response<'_>| {
            if let Some(refused) = flush.admin_only(req)? {
                return resp.send(refused.status, JSON, refused.body.as_bytes());
            }
            held.invalidate();
            let done = cache_flushed();
            resp.send(done.status, JSON, done.body.as_bytes())
        },
    )
}
