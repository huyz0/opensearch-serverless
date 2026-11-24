//! `_index_template` and `_component_template`: store, read and remove the
//! configuration a new index inherits, over the document sets.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/TemplateHandler.java`,
//! over `oss-meta`'s [`DocSet`] and `oss-coordinator`'s `template_admin`.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::template_admin::{
    ACKNOWLEDGED, Kind, concrete_name, render, select, validate,
};
use oss_format::{DocKind, MAX_DOC_BYTES};
use oss_meta::{DocSet, DocSetCache, MetaError};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::route_util::{read_body, refuse, refused};

/// The query parameters a template request accepts and has nothing to do
/// for: there is no cluster manager to time out against.
const PARAMS: &[&str] = &[
    "create",
    "cause",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
    "flat_settings",
    "local",
];

struct Set {
    docs: DocSet,
    cache: DocSetCache,
}

/// Serves the template routes over the two template sets.
pub struct TemplateService {
    index: Set,
    component: Set,
    buffer_bytes: usize,
}

impl TemplateService {
    /// A service over the sets under `base` in `store`; each request holds
    /// two buffers of `buffer_bytes`, charged to the request-body pool.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base that is not empty and does not end in `/`.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        buffer_bytes: usize,
    ) -> Result<Self, MetaError> {
        let set = |kind| -> Result<Set, MetaError> {
            Ok(Set {
                docs: DocSet::new(Arc::clone(store), base, kind)?,
                cache: DocSetCache::new(),
            })
        };
        Ok(Self {
            index: set(DocKind::IndexTemplate)?,
            component: set(DocKind::ComponentTemplate)?,
            buffer_bytes,
        })
    }

    fn handle(
        &self,
        kind: Kind,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let set = match kind {
            Kind::Index => &self.index,
            Kind::Component => &self.component,
        };
        let name = req.path_param("name").map(|n| n.into_owned());
        let method = req.method();
        match method {
            Method::Put | Method::Post => {
                let name = name.ok_or_else(|| RestError::illegal_argument("a name is required"))?;
                concrete_name(&name).map_err(refused)?;
                let body = read_body(req, MAX_DOC_BYTES, "a template is larger than 1 MiB")?;
                if body.is_empty() {
                    return Err(refuse(400, "missing_body", "a template body is required"));
                }
                validate(kind, &body).map_err(refused)?;
                let create = req.param("create").is_some_and(|v| &*v == "true");
                req.body().reserve(self.buffer_bytes)?;
                // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                set.docs
                    .put(&name, &body, create, &mut marker)
                    .map_err(|e| failed(kind, &name, &e))?;
                resp.send(200, JSON, ACKNOWLEDGED.as_bytes())
            }
            Method::Delete => {
                let name = name.ok_or_else(|| RestError::illegal_argument("a name is required"))?;
                concrete_name(&name).map_err(refused)?;
                req.body().reserve(self.buffer_bytes)?;
                // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                let there = set
                    .docs
                    .delete(&name, &mut marker)
                    .map_err(|e| failed(kind, &name, &e))?;
                if there {
                    resp.send(200, JSON, ACKNOWLEDGED.as_bytes())
                } else {
                    Err(missing(&name))
                }
            }
            _ => {
                req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
                // ALLOC: two scratch buffers a request, buffer_bytes each, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                let mut doc = vec![0u8; self.buffer_bytes];
                let snapshot = set
                    .docs
                    .snapshot(&set.cache, &mut marker, &mut doc)
                    .map_err(|e| failed(kind, name.as_deref().unwrap_or(""), &e))?;
                let found = select(snapshot.docs(), name.as_deref());
                if method == Method::Head {
                    let there = found.is_ok_and(|f| !f.is_empty());
                    return resp.send(if there { 200 } else { 404 }, JSON, b"");
                }
                let found = found.map_err(refused)?;
                resp.send(200, JSON, render(kind, &found).as_bytes())
            }
        }
    }
}

fn missing(name: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(404, "template_missing", format!("no such template: {name}"))
}

/// What a failed store call answers.
fn failed(kind: Kind, name: &str, e: &MetaError) -> RestError {
    let what = match kind {
        Kind::Index => "index template",
        Kind::Component => "component template",
    };
    match e {
        MetaError::AlreadyExists => refuse(
            400,
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("{what} [{name}] already exists"),
        ),
        MetaError::TooManyDocs => refuse(400, "too_many_templates", e.to_string()),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => {
            refuse(400, "illegal_argument_exception", e.to_string())
        }
        // ALLOC: error path; one message.
        other => refuse(500, "template_failure", other.to_string()),
    }
}

/// The template routes: both sets, by `PUT`, `POST`, `GET`, `HEAD` and
/// `DELETE`, and a `GET` of all.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<TemplateService>) -> Result<Router, RouteError> {
    let mut router = router;
    for (kind, one, all) in [
        (Kind::Index, "/_index_template/{name}", "/_index_template"),
        (
            Kind::Component,
            "/_component_template/{name}",
            "/_component_template",
        ),
    ] {
        for method in [
            Method::Put,
            Method::Post,
            Method::Get,
            Method::Head,
            Method::Delete,
        ] {
            let service = Arc::clone(service);
            router = router.route(
                method,
                one,
                PARAMS,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    service.handle(kind, req, resp)
                },
            )?;
        }
        let service = Arc::clone(service);
        router = router.route(
            Method::Get,
            all,
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(kind, req, resp),
        )?;
    }
    Ok(router)
}
