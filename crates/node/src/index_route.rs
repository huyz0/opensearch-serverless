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
use oss_ext::pin::{Pin, ResolvedLimits, parse_pins, render};
use oss_format::IndexRecord;
use oss_meta::{DescriptorStore, MetaError, PitClock, Target, new_id};
use oss_rest::{Exception, JSON, Method, Request, Response, RestError, RouteError, Router};
use oss_writer::bucket::policy;
use oss_writer::mapping::{Mapping, ParseOptions};

mod backing;
mod created;
mod mapping;

use crate::index_templates::TemplateSets;
use crate::pin_policy::{PinPolicy, with_members};
use crate::route_util::read_body;
pub use created::Created;

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
    nested: bool,
    pins: Option<Arc<dyn PinPolicy>>,
    pipelines: bool,
    search_pipelines: bool,
    created: Option<Created>,
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
            nested: false,
            pins: None,
            pipelines: false,
            search_pipelines: false,
            created: None,
        }
    }

    /// Accepts a `knn_vector` field in a created index's mapping when `on`
    /// (`serverless.knn.enabled`); off, it is refused as unsupported.
    #[must_use]
    pub fn with_knn(mut self, on: bool) -> Self {
        self.knn = on;
        self
    }

    /// Accepts a `nested` object in a created index's mapping when `on`
    /// (`serverless.nested.enabled`); off, it is refused as unsupported.
    #[must_use]
    pub fn with_nested(mut self, on: bool) -> Self {
        self.nested = on;
        self
    }

    /// Lets a create pin analyzer modules under `policy`
    /// ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)).
    /// Without it every pin is refused.
    #[must_use]
    pub fn with_pins(mut self, policy: Arc<dyn PinPolicy>) -> Self {
        self.pins = Some(policy);
        self
    }

    /// The pins a create asks for, checked: refused when this node has no
    /// policy or the user may not pin, and each module read and probed.
    fn pinned(
        &self,
        req: &Request<'_>,
        settings: &[(String, Option<String>)],
    ) -> Result<Vec<(Pin, ResolvedLimits)>, RestError> {
        // ALLOC: the pin keys of one create, bounded by the body.
        let own: Vec<(String, Option<String>)> = settings
            .iter()
            .filter(|(k, _)| is_pin_key(k))
            .cloned()
            .collect();
        let pins = parse_pins(&own).map_err(|e| {
            // ALLOC: error path; one message.
            refuse(400, "illegal_argument_exception", e.to_string())
        })?;
        if pins.is_empty() {
            return Ok(Vec::new());
        }
        let Some(policy) = &self.pins else {
            return Err(refuse(
                400,
                "extension_unavailable",
                "pinned analyzers are not enabled on this node",
            ));
        };
        if !policy.authorised(req.principal()) {
            return Err(refuse(
                403,
                "security_exception",
                "pinning an analyzer module needs the all_access role",
            ));
        }
        policy
            .allowed()
            .map_err(|why| refuse(400, "extension_unavailable", why))?;
        // ALLOC: one entry a pin, at most four.
        let mut out = Vec::with_capacity(pins.len());
        for pin in pins {
            let limits = policy.check(&pin).map_err(|why| {
                refuse(
                    400,
                    "illegal_argument_exception",
                    // ALLOC: error path; one message.
                    format!("analyzer [{}]: {why}", pin.name),
                )
            })?;
            out.push((pin, limits));
        }
        Ok(out)
    }

    /// Accepts `index.default_pipeline` and `index.final_pipeline` at create
    /// when `on` (`serverless.ingest.pipelines.enabled`); off, they are refused,
    /// as every write to the index would fail on them.
    #[must_use]
    pub fn with_pipelines(mut self, on: bool) -> Self {
        self.pipelines = on;
        self
    }

    /// Accepts `index.search.default_pipeline` at create when `on`
    /// (`serverless.search.pipelines.enabled`); off, it is refused.
    #[must_use]
    pub fn with_search_pipelines(mut self, on: bool) -> Self {
        self.search_pipelines = on;
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

    /// The template sets a create layers, when the node reads them.
    pub(crate) fn templates(&self) -> Option<&Arc<TemplateSets>> {
        self.templates.as_ref()
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
        let (_, shards) = self.create_asked(req, &index, asked)?;
        resp.send(200, JSON, create_ack(&index, shards).as_bytes())
    }

    /// Creates `index` as the request `asked` says: the pins split off, the
    /// templates layered, the settings read. The uuid and the shard count.
    pub(crate) fn create_asked(
        &self,
        req: &mut Request<'_>,
        index: &str,
        mut asked: oss_coordinator::indices::create::CreateRequest,
    ) -> Result<(String, u32), RestError> {
        // A setting the node does not read is refused before any request.
        // ALLOC: the non-pin settings of one create, bounded by the body.
        let own: Vec<_> = asked
            .settings
            .iter()
            .filter(|(k, _)| !is_pin_key(k))
            .cloned()
            .collect();
        crate::create_settings::refuse_others(&own, is_bucket_key)?;
        let pins = self.pinned(req, &asked.settings)?;
        asked.settings = own;
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
                let applied = templates
                    .apply(index, asked, &mut marker, &mut doc)
                    .map_err(refusal);
                drop((marker, doc));
                self.unscratch(req);
                let asked = applied?;
                crate::create_settings::refuse_others(&asked.settings, is_bucket_key)?;
                asked
            }
            None => asked,
        };
        let shards = asked.shards;
        let uuid = self.build(req, index, asked, &pins)?;
        self.notify_created(req, index, &uuid);
        Ok((uuid, shards))
    }

    /// Creates the index `index` as `asked` says, after the templates and
    /// pins are settled; the uuid it was created with.
    fn build(
        &self,
        req: &mut Request<'_>,
        index: &str,
        asked: oss_coordinator::indices::create::CreateRequest,
        pins: &[(Pin, ResolvedLimits)],
    ) -> Result<String, RestError> {
        let (bucket_off, bucket_group) = bucket_settings(&asked.settings)?;
        let cost = cost_settings(&asked.settings)?;
        let read = crate::create_settings::checked(
            &asked.settings,
            asked.shards,
            self.pipelines,
            self.search_pipelines,
        )?;
        if let Some(text) = asked.mapping.as_deref() {
            // `index.knn` cannot be given at create yet, so the node setting stands for it.
            let options = ParseOptions {
                knn: self.knn.then_some(true),
                nested: self.nested,
            };
            let parsed = if pins.is_empty() {
                Mapping::parse_with(text.as_bytes(), options)
            } else {
                Mapping::parse_pinned(text.as_bytes(), options)
            }
            .map_err(|e| refuse(400, "mapper_parsing_exception", e.failed_to_parse().reason))?;
            if let Some(name) = parsed
                .ext_analyzer_names()
                .into_iter()
                .find(|n| !pins.iter().any(|(p, _)| &p.name == n))
            {
                return Err(refuse(
                    400,
                    "mapper_parsing_exception",
                    // ALLOC: error path; one message.
                    format!("analyzer [{name}] has not been configured in mappings"),
                ));
            }
        }
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one buffer a create, buffer_bytes, reserved above.
        let mut buf = vec![0u8; self.buffer_bytes];
        let uuid = new_id().map_err(|e| refuse(500, "index_create_failure", e.to_string()))?;
        let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
        let mut record = IndexRecord::new(index, &uuid, asked.shards);
        record.mapping = asked.mapping.as_deref().map(Cow::Borrowed);
        record.created_at_millis = now;
        record.created_version_id = CREATED_VERSION_ID;
        let tenancy = crate::writer::Tenancy {
            group: bucket_group.clone(),
            ..cost
        };
        let rendered = render(pins);
        let tenancy_settings = (!tenancy.is_empty() || !read.is_empty())
            .then(|| crate::writer::tenancy::tenancy_settings_with(&tenancy, &read));
        let settings = (!rendered.is_empty())
            .then(|| with_members(tenancy_settings.as_deref().unwrap_or(b"{}"), &rendered));
        if let Some(settings) = settings.as_ref().or(tenancy_settings.as_ref()) {
            record.settings = settings;
        }
        if asked.shards == 1
            && !bucket_off
            && let Some(join) = &self.join
        {
            let group = bucket_group.as_deref().unwrap_or(index);
            let bucket = join(group, index, &uuid)
                .map_err(|e| refuse(500, "index_create_failure", format!("bucket: {e}")))?;
            record.bucket = Some(Cow::Owned(bucket));
        }
        if let Err(e) = self.descriptors.create(&record, now, &mut buf) {
            // Best effort: a member left behind has no descriptor naming it.
            if let (Some(leave), Some(bucket)) = (&self.leave, record.bucket.as_deref()) {
                let _ = leave(bucket, &uuid);
            }
            return Err(create_failure(index, &uuid, &e));
        }
        Ok(uuid)
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
pub(crate) fn is_bucket_key(key: &str) -> bool {
    key == policy::INDEX_POLICY.key()
        || key == policy::GROUP.key()
        || key == crate::writer::COST_TIER
        || key == crate::writer::COST_BUDGET
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

/// The cost tier and budget a create names; a tier that is none of the
/// three, or a budget that is not a positive whole number, is refused.
fn cost_settings(
    settings: &[(String, Option<String>)],
) -> Result<crate::writer::Tenancy, RestError> {
    let mut out = crate::writer::Tenancy::default();
    for (key, value) in settings {
        let value = value.as_deref().unwrap_or("");
        if key == crate::writer::COST_TIER {
            if crate::tenants::Tier::parse(value).is_none() {
                return Err(refuse(
                    400,
                    "illegal_argument_exception",
                    format!("{key} is [{value}], not small, standard or large"),
                ));
            }
            out.tier = Some(value.to_owned()); // ALLOC: one name a create.
        } else if key == crate::writer::COST_BUDGET {
            match value.trim().parse::<u64>() {
                Ok(budget) if budget > 0 => out.budget = Some(budget),
                _ => {
                    return Err(refuse(
                        400,
                        "illegal_argument_exception",
                        format!("{key} is [{value}], not a positive whole number"),
                    ));
                }
            }
        }
    }
    Ok(out)
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
    let exists = Arc::clone(&service);
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
            move |req: &mut Request<'_>, resp: &mut Response<'_>| exists.exists(req, resp),
        )
        .and_then(|router| mapping::routes(router, &service))
}

/// Whether `key` sets an extension analyzer pin; other analysis settings
/// stay for the create to refuse by name.
fn is_pin_key(key: &str) -> bool {
    key.starts_with("index.analysis.analyzer.ext_")
}
