//! `_bulk` over the shards this node writes: the body streams through the
//! bulk parser in slices charged to the bulk budget, each slice applied to
//! the writer role, and the response is the items' results.

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use oss_rest::{JSON, Length, Method, Request, Response, RestError, RouteError, Router};
use oss_writer::bulk::{Bulk, BulkBudget, Defaults, StreamConfig, stream, write_response};

use crate::id_lanes::IdLanes;
use crate::ingest_run::IngestPipelines;
use crate::update_bulk::RoleUpdates;
use crate::writer::{RoleTarget, WriterRole};

/// The least a bulk with a declared length takes from the budget.
const MIN_GRANT: usize = 64 << 10;

/// The request parameters a bulk takes.
const PARAMS: &[&str] = &["refresh", "routing", "pipeline", "require_alias"];

/// Serves `_bulk` over a [`WriterRole`].
pub struct BulkService {
    role: Arc<WriterRole>,
    budget: BulkBudget,
    config: StreamConfig,
    ids: IdLanes,
    ingest: Option<Arc<IngestPipelines>>,
    scripts: Option<Arc<dyn oss_types::stored_scripts::StoredScripts>>,
}

/// A request body as a reader: each chunk is copied out whole, so the buffer
/// it reads into holds the body's largest chunk.
struct BodyRead<'r, 'a> {
    body: &'r mut oss_rest::Body<'a>,
    failure: Option<RestError>,
}

impl Read for BodyRead<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.body.next_chunk() {
            Ok(Some(chunk)) => {
                let n = chunk.len().min(buf.len());
                if let (Some(to), Some(from)) = (buf.get_mut(..n), chunk.get(..n)) {
                    to.copy_from_slice(from);
                }
                Ok(n)
            }
            Ok(None) => Ok(0),
            Err(e) => {
                self.failure = Some(e);
                Err(io::Error::other("the request body could not be read"))
            }
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// `?refresh`: whether the shards written are refreshed before the answer.
pub(crate) fn refresh(value: Option<&str>) -> Result<bool, RestError> {
    match value {
        None | Some("false") => Ok(false),
        Some("" | "true" | "wait_for") => Ok(true),
        Some(other) => Err(RestError::illegal_argument(format!(
            "Failed to parse value [{other}] as only [true], [false], [wait_for] or [] are allowed."
        ))),
    }
}

impl BulkService {
    /// A service applying bulks to `role`, its slices charged to `budget`,
    /// ids generated from `address`.
    pub fn new(
        role: Arc<WriterRole>,
        budget: BulkBudget,
        config: StreamConfig,
        address: [u8; 6],
    ) -> Self {
        Self {
            role,
            budget,
            config,
            ids: IdLanes::new(address),
            ingest: None,
            scripts: None,
        }
    }

    /// The same, resolving a script named by `id` in an update item through
    /// `scripts`.
    #[must_use]
    pub fn with_scripts(
        mut self,
        scripts: Arc<dyn oss_types::stored_scripts::StoredScripts>,
    ) -> Self {
        self.scripts = Some(scripts);
        self
    }

    /// The same service running the pipelines its items name, from `ingest`
    /// when there is one.
    #[must_use]
    pub fn with_ingest(mut self, ingest: Option<Arc<IngestPipelines>>) -> Self {
        self.ingest = ingest;
        self
    }

    /// Applies `body`, a `_bulk` body held in memory, as a request with
    /// `defaults` would be, and answers its items.
    ///
    /// # Errors
    ///
    /// What `_bulk` refuses for the same bytes.
    pub(crate) fn apply(
        &self,
        defaults: &Defaults,
        body: &[u8],
    ) -> Result<Vec<oss_writer::bulk::Item>, RestError> {
        let mut buffer = vec![0u8; body.len().clamp(4096, 1 << 20)];
        let mut target = RoleTarget::new(&self.role);
        let mut ids = self.ids.generator();
        let mut updates = RoleUpdates::new(&self.role, self.scripts.as_deref());
        let mut run =
            Bulk::new(&mut target, &mut ids, now_millis(), false).with_updates(&mut updates);
        let mut reader = body;
        stream(
            &mut reader,
            &mut buffer,
            defaults,
            &self.budget,
            self.config,
            &mut run,
        )?;
        run.finish()
    }

    /// Handles one `_bulk`.
    pub fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let started = Instant::now();
        let defaults = Defaults {
            index: req.path_param("index").map(|i| i.into_owned()),
            routing: req.param("routing").map(|v| v.into_owned()),
            pipeline: req.param("pipeline").map(|v| v.into_owned()),
            require_alias: req.param("require_alias").is_some_and(|v| v == "true"),
        };
        let refresh = refresh(req.param("refresh").as_deref())?;
        let mut config = self.config;
        let declared = req.body().declared_length();
        if let Some(declared) = declared.filter(|_| self.role.pipeline_depth() > 1) {
            // With a pipeline, a small body holds a small share of the
            // budget, so many of them wait for the log together; the parsed
            // items take about twice the body. Without one, a bulk keeps
            // its whole slice.
            let held = usize::try_from(declared)
                .unwrap_or(usize::MAX)
                .saturating_mul(2)
                .max(MIN_GRANT);
            config.slice_bytes = config.slice_bytes.min(held);
        }
        let chunk = req.body().max_chunk().max(4096);
        // ALLOC: one read buffer a bulk, the body's largest chunk.
        let mut buffer = vec![0u8; chunk];
        let mut target = RoleTarget::new(&self.role);
        // Its own generator: no lock is held while the request waits for
        // its log commit.
        let mut ids = self.ids.generator();
        let mut pipelines = match &self.ingest {
            Some(set) => {
                req.body().reserve(set.reserve_bytes())?;
                Some(set.run())
            }
            None => None,
        };
        let mut updates = RoleUpdates::new(&self.role, self.scripts.as_deref());
        let mut run =
            Bulk::new(&mut target, &mut ids, now_millis(), refresh).with_updates(&mut updates);
        if let Some(pipelines) = pipelines.as_mut() {
            run = run.with_ingest(pipelines);
        }
        let mut body = BodyRead {
            body: req.body(),
            failure: None,
        };
        let streamed = stream(
            &mut body,
            &mut buffer,
            &defaults,
            &self.budget,
            config,
            &mut run,
        );
        if let Some(e) = body.failure.take() {
            return Err(e);
        }
        streamed?;
        let items = run.finish()?;
        let took = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut out = resp.start(200, Some(JSON), Length::Chunked, &[])?;
        write_response(&mut out, took, &items)?;
        out.flush()?;
        out.finish()
    }
}

/// The routes of `_bulk`: with and without an index, by POST and PUT.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<BulkService>) -> Result<Router, RouteError> {
    let mut router = router;
    for method in [Method::Post, Method::Put] {
        for path in ["/_bulk", "/{index}/_bulk"] {
            let service = Arc::clone(&service);
            router = router.route(
                method,
                path,
                PARAMS,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp),
            )?;
        }
    }
    Ok(router)
}
