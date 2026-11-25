//! `_search/pipeline`: store, read and remove search pipelines over the
//! document set, and hand a search the pipeline it names.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/SearchPipelineHandler.java`
//! (its put, get and delete routes), over `oss-meta`'s [`DocSet`] and
//! `oss-coordinator`'s `pipeline_admin`. The same set answers a search's
//! `?search_pipeline=` through [`PipelineSource`]: one marker GET when the
//! set is unchanged, never a LIST.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::pipeline_admin::{concrete_id, render, select, validate_search};
use oss_format::{DocKind, MAX_DOC_BYTES};
use oss_meta::{DocSet, DocSetCache, MetaError};
use oss_reader::search_pipeline::SearchPipeline;
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::route_util::{read_body, refuse, refused};
use crate::search::{PipelineFault, PipelineSource};

/// The query parameters a pipeline request accepts and has nothing to do for.
const PARAMS: &[&str] = &["master_timeout", "cluster_manager_timeout", "timeout"];

const ACKNOWLEDGED: &str = "{\"acknowledged\":true}";

/// The search pipeline set and what the node keeps of it.
pub struct SearchPipelines {
    docs: DocSet,
    cache: DocSetCache,
    buffer_bytes: usize,
}

impl SearchPipelines {
    /// The set under `base` in `store`; a read holds two buffers of
    /// `buffer_bytes`, which the caller reserves first.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base that is not empty and does not end in `/`.
    pub fn open(
        store: &Arc<dyn BlobStore>,
        base: &str,
        buffer_bytes: usize,
    ) -> Result<Arc<Self>, MetaError> {
        // ALLOC: one holder, once, at boot.
        Ok(Arc::new(Self {
            docs: DocSet::new(Arc::clone(store), base, DocKind::SearchPipeline)?,
            cache: DocSetCache::new(),
            buffer_bytes,
        }))
    }

    fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let id = req.path_param("id").map(|n| n.into_owned());
        match req.method() {
            Method::Put => {
                let id = id.ok_or_else(|| RestError::illegal_argument("an id is required"))?;
                concrete_id(&id).map_err(refused)?;
                let body = read_body(req, MAX_DOC_BYTES, "a pipeline is larger than 1 MiB")?;
                if body.is_empty() {
                    return Err(refuse(400, "missing_body", "a pipeline body is required"));
                }
                validate_search(&body).map_err(refused)?;
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
                req.body().reserve(self.reserve_bytes())?;
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

impl PipelineSource for SearchPipelines {
    fn reserve_bytes(&self) -> usize {
        self.buffer_bytes.saturating_mul(2)
    }

    fn resolve(&self, id: &str) -> Result<SearchPipeline, PipelineFault> {
        // ALLOC: two scratch buffers a search that names a pipeline, reserved by the caller.
        let mut marker = vec![0u8; self.buffer_bytes];
        // ALLOC: as above.
        let mut doc = vec![0u8; self.buffer_bytes];
        let snapshot = self
            .docs
            .snapshot(&self.cache, &mut marker, &mut doc)
            // ALLOC: error path; one message.
            .map_err(|e| PipelineFault::Unreadable(e.to_string()))?;
        let body = snapshot.get(id).ok_or(PipelineFault::Missing)?;
        SearchPipeline::compile(body).map_err(|e| {
            // ALLOC: error path; one message.
            PipelineFault::Broken(format!("search pipeline [{id}] cannot run: {e:?}"))
        })
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

/// The pipeline routes: `PUT`, `GET` and `DELETE` of one id, and a `GET` of
/// all.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<SearchPipelines>) -> Result<Router, RouteError> {
    let mut router = router;
    for method in [Method::Put, Method::Get, Method::Delete] {
        let service = Arc::clone(service);
        router = router.route(
            method,
            "/_search/pipeline/{id}",
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp),
        )?;
    }
    let service = Arc::clone(service);
    router.route(
        Method::Get,
        "/_search/pipeline",
        PARAMS,
        move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp),
    )
}
