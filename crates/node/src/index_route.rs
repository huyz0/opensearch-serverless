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
use oss_writer::bucket::policy;
use oss_writer::mapping::{Mapping, ParseOptions};

use crate::index_templates::TemplateSets;
use crate::route_util::read_body;

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

/// Places a new one-shard index in a bucket: its group (the
/// `index.serverless.bucket_group` setting, else its name), name and uuid in,
/// the bucket's id out.
pub type BucketJoin = Arc<dyn Fn(&str, &str, &str) -> Result<String, String> + Send + Sync>;

/// Takes an index back out of the bucket it joined: its bucket and uuid in.
pub type BucketLeave = Arc<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync>;

/// Serves the index admin routes over the descriptors.
pub struct IndexService {
    descriptors: DescriptorStore,
    clock: PitClock,
    buffer_bytes: usize,
    join: Option<BucketJoin>,
    leave: Option<BucketLeave>,
    templates: Option<Arc<TemplateSets>>,
    knn: bool,
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
            join: None,
            leave: None,
            templates: None,
            knn: false,
        }
    }

    /// Accepts a `knn_vector` field in a created index's mapping when `on`
    /// (`serverless.knn.enabled`); off, it is refused as unsupported.
    #[must_use]
    pub fn with_knn(mut self, on: bool) -> Self {
        self.knn = on;
        self
    }

    /// Places each new one-shard index in a bucket through `join`, and
    /// takes it back out through `leave` when its create then fails.
    #[must_use]
    pub fn with_bucket_join(mut self, join: BucketJoin, leave: BucketLeave) -> Self {
        self.join = Some(join);
        self.leave = Some(leave);
        self
    }

    /// Layers the matching index template under each create. Without this
    /// a create inherits nothing and makes no template request.
    #[must_use]
    pub fn with_templates(mut self, templates: Arc<TemplateSets>) -> Self {
        self.templates = Some(templates);
        self
    }

    /// `PUT /{index}`.
    pub fn create(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let index = req
            .path_param("index")
            .ok_or_else(|| RestError::illegal_argument("an index is required"))?;
        check_name(&index, true).map_err(refusal)?;
        let body = read_body(
            req,
            MAX_CREATE_BYTES,
            "the body of a create is larger than 1 MiB",
        )?;
        let asked = parse(1, Some(&body)).map_err(refusal)?;
        let (bucket_off, bucket_group) = bucket_settings(&asked.settings)?;
        if asked.settings.iter().any(|(k, _)| !is_bucket_key(k)) {
            return Err(unsupported(
                "index settings at create are not supported by this node yet: create the index, then update its settings",
            ));
        }
        if !asked.aliases.is_empty() {
            return Err(unsupported(
                "aliases at create are not supported by this node yet: create the index, then add them",
            ));
        }
        let asked = match &self.templates {
            Some(templates) => {
                req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
                // ALLOC: two scratch buffers a create, buffer_bytes each, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                let mut doc = vec![0u8; self.buffer_bytes];
                let asked = templates
                    .apply(&index, asked, &mut marker, &mut doc)
                    .map_err(refusal)?;
                if !asked.settings.is_empty() {
                    return Err(unsupported(
                        "the matching index template carries settings, which this node does not apply at create yet",
                    ));
                }
                asked
            }
            None => asked,
        };
        if let Some(text) = asked.mapping.as_deref() {
            // `index.knn` cannot be given at create yet, so the node setting stands for it.
            Mapping::parse_with(
                text.as_bytes(),
                ParseOptions {
                    knn: self.knn.then_some(true),
                },
            )
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
        if asked.shards == 1
            && !bucket_off
            && let Some(join) = &self.join
        {
            let group = bucket_group.as_deref().unwrap_or(&index);
            let bucket = join(group, &index, &uuid)
                .map_err(|e| refuse(500, "index_create_failure", format!("bucket: {e}")))?;
            record.bucket = Some(Cow::Owned(bucket));
        }
        if let Err(e) = self.descriptors.create(&record, now, &mut buf) {
            // Best effort: a member left behind has no descriptor naming it.
            if let (Some(leave), Some(bucket)) = (&self.leave, record.bucket.as_deref()) {
                let _ = leave(bucket, &uuid);
            }
            return Err(create_failure(&index, &uuid, &e));
        }
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

/// Whether `key` is one of the two bucket settings a create accepts.
fn is_bucket_key(key: &str) -> bool {
    key == policy::INDEX_POLICY.key() || key == policy::GROUP.key()
}

/// What a create's bucket settings say: whether the index opts out of
/// buckets, and the group it names.
fn bucket_settings(
    settings: &[(String, Option<String>)],
) -> Result<(bool, Option<String>), RestError> {
    let mut off = false;
    let mut group = None;
    for (key, value) in settings {
        let value = value.as_deref().unwrap_or("");
        if key == policy::INDEX_POLICY.key() {
            policy::check(value)
                .map_err(|e| refuse(400, "illegal_argument_exception", e.to_string()))?;
            off = value.eq_ignore_ascii_case("off");
        } else if key == policy::GROUP.key() && !value.is_empty() {
            // ALLOC: error-free path; one group name a create.
            group = Some(value.to_owned());
        }
    }
    Ok((off, group))
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
        // ALLOC: error path; one message.
        MetaError::Format(f) => refuse(400, "illegal_argument_exception", f.to_string()),
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
