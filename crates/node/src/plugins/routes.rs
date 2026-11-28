//! Mounting a ready plugin's routes and serving a request through the
//! plugin's guard (docs/specs/plugin-system AC-5, AC-7).
//!
//! The handler reads the body within the point's ceiling, builds the `rest`
//! call's bytes, calls the plugin through its guard, validates the answer
//! with the codec and sends it. It passes no credentials: only a short
//! allow-list of request headers reaches the plugin.

use std::sync::Arc;

use oss_ext::native::GuardError;
use oss_ext::rest::{CodecError, Method as PluginMethod, RestRequest, RestResponse};
use oss_rest::{
    Exception, Handler, Length, Method, Request, Response, RestError, RouteError, Router,
};

use super::registry::{Plugin, Registry, State};

/// The request headers a plugin sees. Nothing that carries a credential.
const PASSED: &[&str] = &["content-type", "accept", "x-opaque-id"];

/// The response headers a plugin may not set: the connection's own framing
/// and credentials.
const WITHHELD: &[&str] = &[
    "content-length",
    "transfer-encoding",
    "connection",
    "set-cookie",
    "content-type",
];

fn refuse(status: u16, kind: &'static str, reason: impl Into<String>) -> RestError {
    RestError::exception(status, Exception::new(kind, reason))
}

struct PluginRoute {
    plugin: Arc<Plugin>,
    action: String,
    method: PluginMethod,
    path_names: Vec<String>,
    query: Vec<String>,
    now: fn() -> u64,
}

fn failed(e: &GuardError) -> RestError {
    let reason = match e {
        GuardError::PluginBusy | GuardError::TenantBusy => "the plugin is at its concurrency limit",
        GuardError::BreakerOpen => "the plugin is unavailable; its breaker is open",
        GuardError::Panicked => "the plugin failed",
        GuardError::Failed(c) => c.detail(),
    };
    refuse(e.http_status(), e.code(), reason)
}

fn malformed(why: CodecError) -> RestError {
    refuse(
        502,
        "plugin_malformed",
        format!("the plugin's answer is not a rest response: {why:?}"),
    )
}

impl PluginRoute {
    fn body(req: &mut Request<'_>, max: usize) -> Result<Vec<u8>, RestError> {
        let body = req.body();
        let mut out: Vec<u8> = Vec::new();
        loop {
            let chunk = body.max_chunk();
            let more = out
                .len()
                .saturating_add(chunk)
                .saturating_sub(body.reserved());
            body.reserve(more)?;
            let Some(next) = body.next_chunk()? else {
                break;
            };
            if out.len().saturating_add(next.len()) > max {
                return Err(refuse(
                    413,
                    "plugin_body_too_large",
                    "the request body is over the plugin's ceiling",
                ));
            }
            // ALLOC: the body so far, at most `max`, reserved above.
            out.extend_from_slice(next);
        }
        Ok(out)
    }

    fn call_bytes(&self, req: &mut Request<'_>, max_in: usize) -> Result<Vec<u8>, RestError> {
        let body = Self::body(req, max_in)?;
        // ALLOC: at most the route's declared parameters and the pass list.
        let pairs = |names: &[String], get: &dyn Fn(&str) -> Option<String>| {
            names
                .iter()
                .filter_map(|n| get(n).map(|v| (n.clone(), v)))
                .collect::<Vec<_>>()
        };
        let path_params = pairs(&self.path_names, &|n| {
            req.path_param(n).map(std::borrow::Cow::into_owned)
        });
        let query = pairs(&self.query, &|n| {
            req.param(n).map(std::borrow::Cow::into_owned)
        });
        let headers = req
            .headers()
            .filter(|(n, _)| PASSED.iter().any(|p| n.eq_ignore_ascii_case(p)))
            .map(|(n, v)| (n.to_ascii_lowercase(), v.to_owned()))
            .collect();
        Ok(RestRequest {
            method: self.method,
            tenant: req.principal().unwrap_or_default().to_owned(),
            action: self.action.clone(),
            path_params,
            query,
            headers,
            body,
        }
        .encode())
    }
}

impl Handler for PluginRoute {
    fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let Some(spec) = oss_ext::point("rest") else {
            return Err(refuse(
                500,
                "plugin_failed",
                "the rest point is not defined",
            ));
        };
        let (max_in, max_out) = (spec.max_in as usize, spec.max_out as usize);
        let input = self.call_bytes(req, max_in)?;
        let tenant = req.principal().unwrap_or_default().to_owned();
        let out = self
            .plugin
            .call(&tenant, "rest", &input, max_out, self.now)
            .map_err(|e| failed(&e))?;
        let answer = RestResponse::decode(&out, max_out).map_err(malformed)?;
        let content_type = answer
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
            .map_or(oss_rest::JSON, |(_, v)| v.as_str());
        let headers: Vec<(&str, &str)> = answer
            .headers
            .iter()
            .filter(|(n, _)| !WITHHELD.iter().any(|w| n.eq_ignore_ascii_case(w)))
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let len = u64::try_from(answer.body.len()).unwrap_or(u64::MAX);
        let mut w = resp.start(
            answer.status,
            Some(content_type),
            Length::Known(len),
            &headers,
        )?;
        std::io::Write::write_all(&mut w, &answer.body)?;
        w.finish()
    }
}

fn method_of(m: PluginMethod) -> Method {
    match m {
        PluginMethod::Get => Method::Get,
        PluginMethod::Post => Method::Post,
        PluginMethod::Put => Method::Put,
        PluginMethod::Delete => Method::Delete,
        PluginMethod::Head => Method::Head,
    }
}

fn leak(text: String) -> &'static str {
    // LEAK: once at start, one string a declared route or parameter; a policy
    // holds at most 64 plugins of 64 routes of 16 parameters, so at most 70,000.
    Box::leak(text.into_boxed_str())
}

fn path_names(pattern: &str) -> Vec<String> {
    pattern
        .split('/')
        .filter_map(|s| s.strip_prefix('{').and_then(|r| r.strip_suffix('}')))
        .map(str::to_owned)
        .collect()
}

/// Mounts every ready plugin's routes on `router`. `now` is the clock the
/// plugins' breakers read, in milliseconds that never go back.
///
/// # Errors
///
/// A route the router refuses (a duplicate it holds that the registry did
/// not know of, or a pattern it cannot parse).
pub fn mount(
    mut router: Router,
    registry: &Registry,
    now: fn() -> u64,
) -> Result<Router, RouteError> {
    for plugin in registry.plugins() {
        if *plugin.state() != State::Ready {
            continue;
        }
        let Some(manifest) = plugin.manifest() else {
            continue;
        };
        for r in manifest.routes() {
            let Some(method) = PluginMethod::parse(r.method()) else {
                continue;
            };
            let pattern = leak(r.path().to_owned());
            let params: Vec<&'static str> = r.query().iter().cloned().map(leak).collect();
            let params: &'static [&'static str] = Box::leak(params.into_boxed_slice());
            let route = PluginRoute {
                plugin: Arc::clone(plugin),
                action: r.action().to_owned(),
                method,
                path_names: path_names(r.path()),
                query: r.query().to_vec(),
                now,
            };
            router = router.route(method_of(method), pattern, params, route)?;
        }
    }
    Ok(router)
}
