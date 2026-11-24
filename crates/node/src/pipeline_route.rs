//! `_ingest/pipeline`: store, read and remove ingest pipelines over the
//! document set.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/PipelineHandler.java`
//! (its put, get and delete routes), over `oss-meta`'s [`DocSet`] and
//! `oss-coordinator`'s `pipeline_admin`. Simulation is not served.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::pipeline_admin::{concrete_id, render, select, validate};
use oss_format::{DocKind, MAX_DOC_BYTES};
use oss_meta::{DocSet, DocSetCache, MetaError};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::route_util::{read_body, refuse, refused};

/// The query parameters a pipeline request accepts and has nothing to do for.
const PARAMS: &[&str] = &["master_timeout", "cluster_manager_timeout", "timeout"];

const ACKNOWLEDGED: &str = "{\"acknowledged\":true}";

/// Serves the pipeline routes over the ingest pipeline set.
pub struct PipelineService {
    docs: DocSet,
    cache: DocSetCache,
    buffer_bytes: usize,
}

impl PipelineService {
    /// A service over the set under `base` in `store`; each request holds up
    /// to two buffers of `buffer_bytes`, charged to the request-body pool.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base that is not empty and does not end in `/`.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        buffer_bytes: usize,
    ) -> Result<Self, MetaError> {
        Ok(Self {
            docs: DocSet::new(Arc::clone(store), base, DocKind::IngestPipeline)?,
            cache: DocSetCache::new(),
            buffer_bytes,
        })
    }

    fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let id = req.path_param("id").map(|n| n.into_owned());
        match req.method() {
            Method::Put | Method::Post => {
                let id = id.ok_or_else(|| RestError::illegal_argument("an id is required"))?;
                concrete_id(&id).map_err(refused)?;
                let body = read_body(req, MAX_DOC_BYTES, "a pipeline is larger than 1 MiB")?;
                if body.is_empty() {
                    return Err(refuse(400, "missing_body", "a pipeline body is required"));
                }
                validate(&body).map_err(refused)?;
                req.body().reserve(self.buffer_bytes)?;
                // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                self.docs
                    .put(&id, &body, false, &mut marker)
                    .map_err(|e| failed(&e))?;
                resp.send(200, JSON, ACKNOWLEDGED.as_bytes())
            }
            Method::Delete => {
                let id = id.ok_or_else(|| RestError::illegal_argument("an id is required"))?;
                concrete_id(&id).map_err(refused)?;
                req.body().reserve(self.buffer_bytes)?;
                // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                if self.docs.delete(&id, &mut marker).map_err(|e| failed(&e))? {
                    resp.send(200, JSON, ACKNOWLEDGED.as_bytes())
                } else {
                    // ALLOC: error path; one message.
                    Err(refuse(
                        404,
                        "pipeline_missing",
                        format!("no such pipeline: {id}"),
                    ))
                }
            }
            _ => {
                req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
                // ALLOC: two scratch buffers a request, buffer_bytes each, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                let mut doc = vec![0u8; self.buffer_bytes];
                let snapshot = self
                    .docs
                    .snapshot(&self.cache, &mut marker, &mut doc)
                    .map_err(|e| failed(&e))?;
                let found = select(snapshot.docs(), id.as_deref()).map_err(refused)?;
                resp.send(200, JSON, render(&found).as_bytes())
            }
        }
    }
}

/// What a failed store call answers.
fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyDocs => refuse(400, "too_many_pipelines", e.to_string()),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => {
            refuse(400, "illegal_argument_exception", e.to_string())
        }
        // ALLOC: error path; one message.
        other => refuse(500, "pipeline_failure", other.to_string()),
    }
}

/// The pipeline routes: `PUT`, `POST`, `GET` and `DELETE` of one id, and a
/// `GET` of all.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<PipelineService>) -> Result<Router, RouteError> {
    let mut router = router;
    for method in [Method::Put, Method::Post, Method::Get, Method::Delete] {
        let service = Arc::clone(service);
        router = router.route(
            method,
            "/_ingest/pipeline/{id}",
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp),
        )?;
    }
    let service = Arc::clone(service);
    router.route(
        Method::Get,
        "/_ingest/pipeline",
        PARAMS,
        move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp),
    )
}
