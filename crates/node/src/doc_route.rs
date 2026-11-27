//! The single-document write APIs over the shards this node writes: each
//! request is one item run through the bulk machinery, so planning, version
//! rules, the write log and shard routing are the `_bulk` path's.

use std::io::Write;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_rest::{JSON, Length, Method, Request, Response, RestError, RouteError, Router};
use oss_writer::bulk::{Action, Bulk, Kind, Op, doc_status, write_doc};
use oss_writer::index::{Expect, MATCH_ANY, VersionType};

use crate::bulk_route::refresh;
use crate::id_lanes::IdLanes;
use crate::ingest_run::IngestPipelines;
use crate::writer::{RoleTarget, WriterRole};

/// What an index or create request takes.
const WRITE_PARAMS: &[&str] = &[
    "refresh",
    "routing",
    "op_type",
    "version",
    "version_type",
    "if_seq_no",
    "if_primary_term",
    "pipeline",
    "require_alias",
];

/// What a delete takes.
const DELETE_PARAMS: &[&str] = &[
    "refresh",
    "routing",
    "version",
    "version_type",
    "if_seq_no",
    "if_primary_term",
];

/// `if_seq_no` when a request sets none.
const UNASSIGNED: i64 = -2;

/// Serves the document write APIs over a [`WriterRole`].
pub struct DocService {
    role: Arc<WriterRole>,
    ids: IdLanes,
    ingest: Option<Arc<IngestPipelines>>,
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A numeric parameter, or the failure OpenSearch answers a bad one with.
pub(crate) fn number(req: &Request<'_>, name: &str) -> Result<Option<i64>, RestError> {
    match req.param(name) {
        None => Ok(None),
        Some(text) => text.parse::<i64>().map(Some).map_err(|_| {
            // ALLOC: error path; one message.
            RestError::illegal_argument(format!(
                "Failed to parse int parameter [{name}] with value [{text}]"
            ))
        }),
    }
}

/// The concurrency conditions a request sets.
fn expect(req: &Request<'_>) -> Result<Expect, RestError> {
    let version_type = match req.param("version_type") {
        None => VersionType::Internal,
        Some(text) => VersionType::parse(&text).ok_or_else(|| {
            // ALLOC: error path; one message.
            RestError::illegal_argument(format!("No version type match [{text}]"))
        })?,
    };
    Ok(Expect {
        version: number(req, "version")?.unwrap_or(MATCH_ANY),
        version_type,
        if_seq_no: number(req, "if_seq_no")?.unwrap_or(UNASSIGNED),
        if_primary_term: number(req, "if_primary_term")?.unwrap_or(0),
    })
}

impl DocService {
    /// A service applying writes to `role`, ids generated from `address`.
    pub fn new(role: Arc<WriterRole>, address: [u8; 6]) -> Self {
        Self {
            role,
            ids: IdLanes::new(address),
            ingest: None,
        }
    }

    /// The same service running the pipeline a request names, from `ingest`
    /// when there is one.
    #[must_use]
    pub fn with_ingest(mut self, ingest: Option<Arc<IngestPipelines>>) -> Self {
        self.ingest = ingest;
        self
    }

    /// The request's body, held whole and charged to the request-body pool.
    pub(crate) fn source(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        let declared = req
            .body()
            .declared_length()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        req.body().reserve(declared)?;
        // ALLOC: the document, charged to the request-body pool above and as it grows.
        let mut source = Vec::with_capacity(declared);
        while let Some(chunk) = req.body().next_chunk()? {
            source.extend_from_slice(chunk);
            let read = chunk.len();
            if source.len() > declared {
                req.body().reserve(read)?;
            }
        }
        if source.is_empty() {
            return Err(RestError::illegal_argument("request body is required"));
        }
        Ok(source)
    }

    /// Handles one document write of `kind`.
    fn handle(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        kind: Kind,
    ) -> Result<(), RestError> {
        let refresh = refresh(req.param("refresh").as_deref())?;
        let create = match kind {
            Kind::Create => true,
            Kind::Index => req.param("op_type").is_some_and(|t| t == "create"),
            Kind::Delete | Kind::Update => false,
        };
        let action = Action {
            kind,
            index: req.path_param("index").map(|i| i.into_owned()),
            id: req.path_param("id").map(|i| i.into_owned()),
            routing: req.param("routing").map(|v| v.into_owned()),
            create,
            expect: expect(req)?,
            pipeline: req.param("pipeline").map(|v| v.into_owned()),
            require_alias: req.param("require_alias").is_some_and(|v| v == "true"),
        };
        let source = if kind == Kind::Delete {
            Vec::new()
        } else {
            Self::source(req)?
        };
        let mut target = RoleTarget::new(&self.role);
        let mut ids = self.ids.generator();
        // The index's own default or final pipeline may run whatever the
        // request names, so with pipelines on a write that is not a delete
        // holds the run's buffers, as a bulk does.
        let mut pipelines = match (&self.ingest, kind != Kind::Delete) {
            (Some(set), true) => {
                req.body().reserve(set.reserve_bytes())?;
                Some(set.run())
            }
            _ => None,
        };
        let mut run = Bulk::new(&mut target, &mut ids, now_millis(), refresh);
        if let Some(pipelines) = pipelines.as_mut() {
            run = run.with_ingest(pipelines);
        }
        run.slice(&[Op { action, source }])?;
        let items = run.finish()?;
        let Some(item) = items.first() else {
            return Err(RestError::simple(500, "the write produced no result"));
        };
        let status = doc_status(item);
        let retry_after = (status == 429)
            .then(|| self.role.retry_after_secs())
            .flatten()
            .map(|secs| secs.to_string());
        let headers: Vec<(&str, &str)> = retry_after
            .iter()
            .map(|secs| ("retry-after", secs.as_str()))
            .collect();
        let mut out = resp.start(status, Some(JSON), Length::Chunked, &headers)?;
        write_doc(&mut out, item)?;
        out.flush()?;
        out.finish()
    }
}

/// The routes of the document write APIs.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<DocService>) -> Result<Router, RouteError> {
    let mut router = router;
    let table: [(Method, &str, Kind, &[&str]); 6] = [
        (Method::Put, "/{index}/_doc/{id}", Kind::Index, WRITE_PARAMS),
        (
            Method::Post,
            "/{index}/_doc/{id}",
            Kind::Index,
            WRITE_PARAMS,
        ),
        (Method::Post, "/{index}/_doc", Kind::Index, WRITE_PARAMS),
        (
            Method::Put,
            "/{index}/_create/{id}",
            Kind::Create,
            WRITE_PARAMS,
        ),
        (
            Method::Post,
            "/{index}/_create/{id}",
            Kind::Create,
            WRITE_PARAMS,
        ),
        (
            Method::Delete,
            "/{index}/_doc/{id}",
            Kind::Delete,
            DELETE_PARAMS,
        ),
    ];
    for (method, path, kind, params) in table {
        let service = Arc::clone(&service);
        router = router.route(
            method,
            path,
            params,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp, kind),
        )?;
    }
    Ok(router)
}
