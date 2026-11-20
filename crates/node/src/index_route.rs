//! Index admin over REST: create an index and ask whether a name holds one.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/indexadmin/IndexAdminHandler.java`
//! (its create and exists routes), over `oss-meta`'s descriptor store and
//! `oss-coordinator`'s decisions.

use std::borrow::Cow;
use std::sync::Arc;

use oss_coordinator::indices::admin::{check_name, create_ack};
use oss_coordinator::indices::create::parse;
use oss_coordinator::route::resolve::Refusal;
use oss_format::IndexRecord;
use oss_meta::{DescriptorStore, MetaError, PitClock, Target, new_id};
use oss_rest::{Exception, JSON, Method, Request, Response, RestError, RouteError, Router};
use oss_writer::mapping::Mapping;

/// The most bytes of a create body.
const MAX_CREATE_BYTES: usize = 1 << 20;

/// The query parameters a create accepts and has nothing to do for: one
/// node, no replicas and no wait.
const CREATE_PARAMS: &[&str] = &[
    "timeout",
    "master_timeout",
    "cluster_manager_timeout",
    "wait_for_active_shards",
];

/// The OpenSearch version id the descriptors this node creates carry.
const CREATED_VERSION_ID: i32 = 0;

fn refuse(status: u16, kind: &'static str, reason: impl Into<String>) -> RestError {
    RestError::exception(status, Exception::new(kind, reason))
}

fn refusal(r: Refusal) -> RestError {
    refuse(r.status, r.kind, r.reason)
}

/// Serves the index admin routes over the descriptors.
pub struct IndexService {
    descriptors: DescriptorStore,
    clock: PitClock,
    buffer_bytes: usize,
}

impl IndexService {
    /// A service over `descriptors`, telling the time by `clock`
    /// (milliseconds). Each create holds one buffer of `buffer_bytes` for
    /// the descriptor, charged to the request-body pool.
    pub fn new(descriptors: DescriptorStore, clock: PitClock, buffer_bytes: usize) -> Self {
        Self {
            descriptors,
            clock,
            buffer_bytes,
        }
    }

    /// The body, read as it streams in, its growth reserved first.
    fn read_body(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        let body = req.body();
        let mut out: Vec<u8> = Vec::new();
        loop {
            let chunk = body.max_chunk();
            let more = out
                .len()
                .saturating_add(chunk)
                .saturating_sub(body.reserved());
            if more > 0 {
                body.reserve(more)?;
            }
            let Some(next) = body.next_chunk()? else {
                break;
            };
            if out.len().saturating_add(next.len()) > MAX_CREATE_BYTES {
                return Err(refuse(
                    400,
                    "illegal_argument_exception",
                    "the body of a create is larger than 1 MiB",
                ));
            }
            // ALLOC: the body so far, at most MAX_CREATE_BYTES, reserved above.
            out.extend_from_slice(next);
        }
        Ok(out)
    }

    /// `PUT /{index}`.
    pub fn create(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let index = req
            .path_param("index")
            .ok_or_else(|| RestError::illegal_argument("an index is required"))?;
        check_name(&index, true).map_err(refusal)?;
        let body = Self::read_body(req)?;
        let asked = parse(1, Some(&body)).map_err(refusal)?;
        if !asked.settings.is_empty() {
            return Err(unsupported(
                "index settings at create are not supported by this node yet: create the index, then update its settings",
            ));
        }
        if !asked.aliases.is_empty() {
            return Err(unsupported(
                "aliases at create are not supported by this node yet: create the index, then add them",
            ));
        }
        if let Some(text) = asked.mapping.as_deref() {
            Mapping::parse(text.as_bytes())
                .map_err(|e| refuse(400, "mapper_parsing_exception", e.failed_to_parse().reason))?;
        }
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one buffer a create, buffer_bytes, reserved above.
        let mut buf = vec![0u8; self.buffer_bytes];
        let uuid = new_id().map_err(|e| refuse(500, "index_create_failure", e.to_string()))?;
        let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
        let mut record = IndexRecord::new(&index, &uuid, asked.shards);
        record.mapping = asked.mapping.as_deref().map(Cow::Borrowed);
        record.created_at_millis = now;
        record.created_version_id = CREATED_VERSION_ID;
        self.descriptors
            .create(&record, now, &mut buf)
            .map_err(|e| create_failure(&index, &uuid, &e))?;
        resp.send(200, JSON, create_ack(&index, asked.shards).as_bytes())
    }

    /// `HEAD /{index}`.
    pub fn exists(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let index = req
            .path_param("index")
            .ok_or_else(|| RestError::illegal_argument("an index is required"))?;
        check_name(&index, false).map_err(refusal)?;
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one buffer a request, buffer_bytes, reserved above.
        let mut buf = vec![0u8; self.buffer_bytes];
        let held = self
            .descriptors
            .resolve(&index, &mut buf)
            .map_err(|e| refuse(500, "index_lookup_failure", e.to_string()))?;
        let status = match held.target {
            Target::Index(_) | Target::Alias(_) => 200,
            Target::Absent => 404,
        };
        resp.send(status, JSON, b"")
    }
}

fn unsupported(reason: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(501, "unsupported_index_option", reason)
}

/// What a failed create answers.
fn create_failure(index: &str, uuid: &str, e: &MetaError) -> RestError {
    match e {
        // ALLOC: error path; one message.
        MetaError::AlreadyExists => refuse(
            400,
            "resource_already_exists_exception",
            format!("index [{index}/{uuid}] already exists"),
        ),
        MetaError::BeingReclaimed => refuse(409, "name_being_reclaimed_exception", e.to_string()),
        MetaError::InvalidName(_) => {
            // ALLOC: error path; one message.
            refuse(
                400,
                "invalid_index_name_exception",
                format!("Invalid index name [{index}], {e}"),
            )
        }
        // ALLOC: error path; one message.
        other => refuse(500, "index_create_failure", other.to_string()),
    }
}

/// The index admin routes registered so far.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<IndexService>) -> Result<Router, RouteError> {
    let create = Arc::clone(&service);
    router
        .route(
            Method::Put,
            "/{index}",
            CREATE_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| create.create(req, resp),
        )?
        .route(
            Method::Head,
            "/{index}",
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.exists(req, resp),
        )
}
