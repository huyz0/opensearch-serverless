//! `GET` and `PUT /_cluster/settings`: the cluster config register at the
//! surface a caller reaches.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/ClusterSettingsHandler.java`.
//! What a body may say and the words of each refusal are
//! `oss_coordinator::cluster::settings`'s; this reads the register and writes
//! it.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::cluster::settings::{Setting, Shown, ack_body, parse_update, settings_body};
use oss_format::{ClusterSettings, MAX_CLUSTER_SETTINGS_BYTES, Setting as Held};
use oss_meta::{ClusterConfig, MetaError};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::route_util::{flag, refuse, refused};

/// The buffer a request reads and writes the register through: written, a
/// control character takes six bytes.
const REGISTER_BUFFER: usize = 6 * MAX_CLUSTER_SETTINGS_BYTES;

/// The most bytes an update body may be.
const MAX_BODY: usize = 1 << 20;

/// What a settings read takes.
const READ_PARAMS: &[&str] = &[
    "include_defaults",
    "flat_settings",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
];

/// Serves the cluster settings routes over the cluster config register.
pub struct ClusterService {
    config: ClusterConfig,
    commit_period_millis: u64,
}

fn shown(settings: &ClusterSettings) -> Vec<(String, Shown)> {
    // ALLOC: one entry a setting, bounded by the register's size limit.
    settings
        .settings()
        .iter()
        .map(|(key, value)| {
            let value = match value {
                Held::Value(v) => Shown::Value(v.clone()),
                Held::List(items) => Shown::List(items.clone()),
            };
            (key.clone(), value)
        })
        .collect()
}

fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::SettingsRefused(_) => {
            // ALLOC: error path; the refusal's words.
            refuse(400, "illegal_argument_exception", e.to_string())
        }
        MetaError::Contended => refuse(
            503,
            "cluster_config_contended",
            "the cluster settings moved under every attempt to write them; retry",
        ),
        // ALLOC: error path; the failure's words.
        _ => refuse(500, "cluster_config_failure", e.to_string()),
    }
}

impl ClusterService {
    /// A service over the register under `base` of `store`, `commit_period_millis`
    /// being the node's commit period, which bounds a checkpoint age.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a base that is not empty and does not end in
    /// `/`.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        commit_period_millis: u64,
    ) -> Result<Self, MetaError> {
        Ok(Self {
            config: ClusterConfig::new(store, base)?,
            commit_period_millis,
        })
    }

    fn buffer(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        req.body().reserve(REGISTER_BUFFER)?;
        // ALLOC: one buffer a request, REGISTER_BUFFER, reserved above.
        Ok(vec![0u8; REGISTER_BUFFER])
    }

    /// `GET /_cluster/settings`.
    pub fn read(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let flat = flag(req, "flat_settings", false);
        let defaults = flag(req, "include_defaults", false);
        let mut buf = Self::buffer(req)?;
        let value = self.config.read(&mut buf).map_err(|e| failed(&e))?;
        let body = settings_body(&shown(&value.settings), flat, defaults);
        resp.send(200, JSON, body.as_bytes())
    }

    /// The request's body, whole, charged to the request-body pool.
    fn body(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        let declared = req
            .body()
            .declared_length()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        if declared > MAX_BODY {
            return Err(RestError::too_large());
        }
        req.body().reserve(declared)?;
        // ALLOC: the body, at most MAX_BODY, reserved above.
        let mut body = Vec::with_capacity(declared);
        while let Some(chunk) = req.body().next_chunk()? {
            if body.len().saturating_add(chunk.len()) > MAX_BODY {
                return Err(RestError::too_large());
            }
            body.extend_from_slice(chunk);
        }
        Ok(body)
    }

    /// `PUT /_cluster/settings`.
    pub fn update(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let flat = flag(req, "flat_settings", false);
        let body = Self::body(req)?;
        let settings: Vec<Setting> = parse_update(
            Some(&body),
            self.commit_period_millis,
            // No plugin owns a setting here.
            &|_: &[Setting]| Ok(()),
        )
        .map_err(refused)?;
        // ALLOC: one pair of references a setting, bounded by the body.
        let changes: Vec<(&str, Option<&str>)> = settings
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_deref()))
            .collect();
        let mut buf = Self::buffer(req)?;
        let written = self
            .config
            .update(&changes, &mut buf)
            .map_err(|e| failed(&e))?;
        let body = ack_body(&shown(&written), flat);
        resp.send(200, JSON, body.as_bytes())
    }
}

/// The routes of the cluster settings.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<ClusterService>) -> Result<Router, RouteError> {
    let read = Arc::clone(&service);
    router
        .route(
            Method::Get,
            "/_cluster/settings",
            READ_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| read.read(req, resp),
        )?
        .route(
            Method::Put,
            "/_cluster/settings",
            READ_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.update(req, resp),
        )
}
