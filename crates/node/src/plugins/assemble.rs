//! Putting the plugin system on a node: read the policy, load the registry
//! against the routes the router already holds, mount the ready plugins and
//! the status route (docs/specs/plugin-system AC-3, AC-11).

use std::collections::HashMap;
use std::sync::Arc;

use oss_ext::host::Backend;
use oss_ext::native::NativeInstance;
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};
use serde_json::{Value, json};

use super::policy::PolicyTier;
use super::registry::{Registry, State, Taken};
use super::{Dispatcher, Policy, mount};

/// The status route's path.
pub const STATUS_PATH: &str = "/_plugins/_serverless/plugins";

/// Why the plugin system could not be put on the node.
#[derive(Debug)]
pub enum InstallError {
    /// The policy text was refused.
    Policy(String),
    /// The router refused a route.
    Route(RouteError),
}

fn tier_name(t: PolicyTier) -> &'static str {
    match t {
        PolicyTier::Native => "native",
        PolicyTier::Wasm => "wasm",
        PolicyTier::Grpc => "grpc",
    }
}

/// The status document: each plugin, whether it serves, why not, and its
/// breaker, and what its event queue has delivered and dropped (null when it
/// subscribed to no event). Answered from memory; it makes no store request.
fn status(registry: &Registry, events: &Dispatcher) -> Value {
    let plugins: Vec<Value> = registry
        .plugins()
        .iter()
        .map(|p| {
            let (state, reason) = match p.state() {
                State::Ready => ("ready", None),
                State::Refused(why) => ("refused", Some(why.as_str())),
            };
            let counts = events
                .counts(p.id())
                .map(|c| json!({ "delivered": c.delivered, "dropped": c.dropped }));
            json!({
                "id": p.id(),
                "tier": tier_name(p.entry().tier),
                "state": state,
                "reason": reason,
                "breaker": format!("{:?}", p.guard().state()).to_ascii_lowercase(),
                "events": counts,
            })
        })
        .collect();
    json!({ "plugins": plugins })
}

/// Reads `policy_text`, loads the plugins it names from `linked`, and mounts
/// their routes and the status route on `router`. `backend` is what the
/// plugins' host operations reach.
///
/// # Errors
///
/// A policy that does not parse, or a route the router refuses.
pub fn install(
    router: Router,
    policy_text: &str,
    linked: &HashMap<String, Arc<dyn NativeInstance>>,
    backend: &Arc<dyn Backend>,
    now: fn() -> u64,
    bus: &crate::lifecycle_bus::Bus,
) -> Result<Router, InstallError> {
    let policy = if policy_text.trim().is_empty() {
        Policy::default()
    } else {
        Policy::parse(policy_text).map_err(|e| InstallError::Policy(e.to_string()))?
    };
    let builtin: Vec<Taken> = router
        .routes()
        .into_iter()
        .map(|(method, pattern)| Taken {
            method: method.map(|m| m.as_str().to_owned()),
            pattern: pattern.to_owned(),
            holder: "a built-in route".to_owned(),
        })
        .collect();
    let registry = Arc::new(Registry::load_with(&policy, linked, &builtin, backend));
    let router = mount(router, &registry, now).map_err(InstallError::Route)?;
    let events = Arc::new(Dispatcher::start(&registry, now));
    bus.set(Arc::clone(&events) as Arc<dyn crate::lifecycle_bus::Events>);
    bus.publish("node.started", "", "", now());
    let seen = Arc::clone(&registry);
    let lanes = Arc::clone(&events);
    router
        .route(
            Method::Get,
            STATUS_PATH,
            &[],
            move |_: &mut Request<'_>, resp: &mut Response<'_>| -> Result<(), RestError> {
                resp.send(200, JSON, status(&seen, &lanes).to_string().as_bytes())
            },
        )
        .map_err(InstallError::Route)
}

#[cfg(feature = "writer")]
/// [`install`] for the node's own assembly: no plugin is linked into this
/// build, the wall clock drives the breakers, and `index.docs` writes through
/// the same service as `_bulk`.
///
/// # Errors
///
/// What [`install`] refuses, as the node's assembly error.
pub(crate) fn install_configured(
    router: Router,
    policy_text: &str,
    bulk: &Arc<crate::bulk_route::BulkService>,
    jobs: Arc<dyn oss_ext::job::JobSink>,
    bus: &crate::lifecycle_bus::Bus,
    modules: &HashMap<String, Arc<dyn NativeInstance>>,
    behind: Arc<dyn Backend>,
) -> Result<Router, crate::assemble_error::AssembleError> {
    use crate::assemble_error::AssembleError;
    use oss_ext::doc_sink::{DocSink, DocSinkBackend};
    use oss_ext::job::JobBackend;
    install(
        router,
        policy_text,
        modules,
        &(Arc::new(JobBackend::new(
            jobs,
            Arc::new(DocSinkBackend::new(
                Arc::clone(bulk) as Arc<dyn DocSink>,
                behind,
            )),
        )) as Arc<dyn Backend>),
        crate::clock::now_millis,
        bus,
    )
    .map_err(|e| match e {
        InstallError::Route(r) => AssembleError::Route(r),
        InstallError::Policy(why) => AssembleError::Plugins(why),
    })
}

/// The WASM modules the node's policy names, read from the store: one GET each
/// at start, none when the policy is empty or names none.
#[cfg(all(feature = "writer", feature = "extensions"))]
pub(crate) fn configured_modules(
    policy_text: &str,
    store: &dyn oss_blob::BlobStore,
    base: &str,
    budget: &oss_mem::MemoryBudget,
) -> HashMap<String, Arc<dyn NativeInstance>> {
    Policy::parse(policy_text).map_or_else(
        |_| HashMap::new(),
        |p| super::wasm_load::load_modules(&p, store, base, budget),
    )
}

/// Without the extension host there are no modules to read.
#[cfg(all(feature = "writer", not(feature = "extensions")))]
pub(crate) fn configured_modules(
    _: &str,
    _: &dyn oss_blob::BlobStore,
    _: &str,
    _: &oss_mem::MemoryBudget,
) -> HashMap<String, Arc<dyn NativeInstance>> {
    HashMap::new()
}
