//! `/_plugins/_alerting`: the alerting plugin's monitor routes, served
//! natively over a tenant's monitor index (ADR 0075).
//!
//! A tenant is the request's `securitytenant` header, else `_default_`. A put
//! is the body's write, one swap of the tenant's index and, for an enabled
//! monitor, one swap of its registry shard; a read is the index and the
//! body; nothing lists. `_seq_no` and `_version` are the index's change
//! count.

mod monitor;
mod routes;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::BlobStore;
use oss_format::{FormatError, MAX_MONITOR_BYTES};
use oss_meta::{MetaError, Monitors, new_id};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use crate::route_util::{read_body, refuse};
use monitor::Checked;
pub use routes::{install, routes};

/// The tenant a request without a security tenant belongs to.
const DEFAULT_TENANT: &str = "_default_";
/// The plugin's config index, shown as every monitor's `_index`.
const INDEX: &str = ".opendistro-alerting-config";
/// The shortest schedule interval, in seconds, unless set otherwise.
const MIN_INTERVAL_SECS: u64 = 60;

/// Serves the monitor routes over one deployment's object store.
pub struct AlertingService {
    store: Arc<dyn BlobStore>,
    base: String,
    min_interval_secs: u64,
}

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn not_found(id: &str) -> RestError {
    refuse(
        404,
        "alerting_exception",
        // ALLOC: error path; one message.
        format!("Monitor not found [{id}]"),
    )
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// What a failed store call answers.
fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::AlreadyExists => refuse(409, "version_conflict_engine_exception", e.to_string()),
        MetaError::Contended => refuse(503, "alerting_exception", e.to_string()),
        // A full index or registry shard.
        MetaError::Format(FormatError::TooLarge { .. }) => {
            refuse(429, "too_many_requests_exception", e.to_string())
        }
        MetaError::Format(_) | MetaError::InvalidArgument(_) => bad(e.to_string()),
        // ALLOC: error path; one message.
        other => refuse(500, "alerting_exception", other.to_string()),
    }
}

impl AlertingService {
    /// A service over the monitors under `base` in `store`.
    pub fn new(store: &Arc<dyn BlobStore>, base: &str) -> Self {
        Self {
            store: Arc::clone(store),
            // ALLOC: one copy of the base, once.
            base: base.to_owned(),
            min_interval_secs: MIN_INTERVAL_SECS,
        }
    }

    /// The shortest schedule interval a monitor may have, in seconds.
    #[must_use]
    pub const fn with_min_interval(mut self, secs: u64) -> Self {
        self.min_interval_secs = secs;
        self
    }

    fn monitors(&self, req: &Request<'_>) -> Result<Monitors, RestError> {
        let tenant = req.header("securitytenant").unwrap_or(DEFAULT_TENANT);
        Monitors::new(&self.store, &self.base, tenant)
            .map_err(|_| bad(format!("invalid tenant [{tenant}]")))
    }

    fn id_of(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("id")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("a monitor id is required"))
    }

    /// The put's answer: the plugin's shape around the stored monitor.
    fn answer(
        resp: &mut Response<'_>,
        status: u16,
        id: &str,
        version: i64,
        monitor: &Value,
    ) -> Result<(), RestError> {
        let body = json!({
            "_id": id,
            "_version": version,
            "_seq_no": version,
            "_primary_term": 1,
            "monitor": monitor,
        });
        resp.send(status, JSON, body.to_string().as_bytes())
    }

    fn checked(&self, req: &mut Request<'_>) -> Result<Checked, RestError> {
        let bytes = read_body(req, MAX_MONITOR_BYTES, "a monitor is over its size bound")?;
        let body: Value =
            serde_json::from_slice(&bytes).map_err(|e| bad(format!("invalid body: {e}")))?;
        Checked::from_put(&body, now_millis(), self.min_interval_secs).map_err(bad)
    }

    fn store_monitor(
        m: &Monitors,
        id: &str,
        checked: &Checked,
        require_absent: bool,
    ) -> Result<i64, RestError> {
        // ALLOC: the monitor's text, at most MAX_MONITOR_BYTES, checked by the put.
        let bytes = checked.stored.to_string().into_bytes();
        let now = now_millis();
        m.put(
            id,
            &bytes,
            checked.enabled,
            &checked.schedule,
            now,
            require_absent,
        )
        .map(|p| p.version)
        .map_err(|e| failed(&e))
    }

    /// `POST /monitors`: a new monitor under a fresh id.
    pub(crate) fn create(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let m = self.monitors(req)?;
        let checked = self.checked(req)?;
        let id = new_id().map_err(|e| refuse(500, "alerting_exception", e.to_string()))?;
        let version = Self::store_monitor(&m, &id, &checked, true)?;
        Self::answer(resp, 201, &id, version, &checked.stored)
    }

    /// `PUT /monitors/{id}`: replaces a monitor that exists.
    pub(crate) fn update(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let (m, id) = (self.monitors(req)?, Self::id_of(req)?);
        let checked = self.checked(req)?;
        if m.index().map_err(|e| failed(&e))?.get(&id).is_none() {
            return Err(not_found(&id));
        }
        let version = Self::store_monitor(&m, &id, &checked, false)?;
        Self::answer(resp, 200, &id, version, &checked.stored)
    }

    /// `GET /monitors/{id}`.
    pub(crate) fn get(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let (m, id) = (self.monitors(req)?, Self::id_of(req)?);
        req.body().reserve(MAX_MONITOR_BYTES)?;
        // ALLOC: one body buffer a request, MAX_MONITOR_BYTES, reserved above.
        let mut buf = vec![0u8; MAX_MONITOR_BYTES];
        let Some((_, n)) = m.get(&id, &mut buf).map_err(|e| failed(&e))? else {
            return Err(not_found(&id));
        };
        let monitor: Value = serde_json::from_slice(buf.get(..n).unwrap_or_default())
            .map_err(|_| refuse(500, "alerting_exception", "a stored monitor is unreadable"))?;
        let version = m.index().map_err(|e| failed(&e))?.version;
        Self::answer(resp, 200, &id, version, &monitor)
    }

    /// `DELETE /monitors/{id}`.
    pub(crate) fn delete(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let (m, id) = (self.monitors(req)?, Self::id_of(req)?);
        if !m.delete(&id).map_err(|e| failed(&e))? {
            return Err(not_found(&id));
        }
        let version = m.index().map_err(|e| failed(&e))?.version;
        let body = json!({
            "_index": INDEX, "_id": id, "_version": version, "result": "deleted",
            "forced_refresh": true,
            "_shards": {"total": 1, "successful": 1, "failed": 0},
            "_seq_no": version, "_primary_term": 1,
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}
