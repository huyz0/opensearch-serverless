//! `/_plugins/_alerting`: the alerting plugin's monitor routes, served
//! natively over a tenant's monitor index (ADR 0075).
//!
//! A tenant is the request's `securitytenant` header, else `_default_`. A put
//! is the body's write, one swap of the tenant's index and, for an enabled
//! monitor, one swap of its registry shard; a read is the index and the
//! body; nothing lists. `_seq_no` and `_version` are the index's change
//! count.

mod alert;
mod alert_routes;
mod execute;
mod misc;
mod monitor;
mod routes;
mod search;
mod search_query;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::BlobStore;
use oss_format::{FormatError, MAX_MONITOR_BYTES};
use oss_meta::{DescriptorStore, MetaError, Monitors, PutGuard, PutOutcome, PutRefusal, new_id};
use oss_rest::{Exception, JSON, Request, Response, RestError};
use serde_json::{Value, json};

use crate::route_util::{read_body, refuse};
use monitor::{Checked, Refused};
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
    descriptors: Option<DescriptorStore>,
    buffer_bytes: usize,
    search: Option<Arc<dyn crate::ad_route::WindowSearch>>,
}

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

/// The plugin's `alerting_exception`, with the Java exception it wraps as
/// the cause.
fn alerting(status: u16, reason: impl Into<String>, cause: Option<String>) -> RestError {
    let mut exception = Exception::new("alerting_exception", reason).with_own_root();
    if let Some(cause) = cause {
        exception = exception.caused_by(Exception::new("exception", cause));
    }
    RestError::exception(status, exception)
}

/// A status exception of an OpenSearch action, as the plugin wraps it.
fn status_cause(reason: &str) -> Option<String> {
    // ALLOC: error path; one message.
    Some(format!(
        "org.opensearch.OpenSearchStatusException: {reason}"
    ))
}

fn not_found() -> RestError {
    alerting(
        404,
        "Monitor not found.",
        status_cause("Monitor not found."),
    )
}

fn refused(r: Refused) -> RestError {
    alerting(r.status, r.reason, r.cause)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// What a failed store call answers.
fn failed(e: &MetaError) -> RestError {
    match e {
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
            descriptors: None,
            buffer_bytes: 0,
            search: None,
        }
    }

    /// Runs a monitor's input through `search` when it is executed.
    #[must_use]
    pub fn with_search(mut self, search: Arc<dyn crate::ad_route::WindowSearch>) -> Self {
        self.search = Some(search);
        self
    }

    /// Checks that the indices a monitor names exist, in `descriptors`, with
    /// `buffer_bytes` for a record.
    #[must_use]
    pub fn with_descriptors(mut self, descriptors: DescriptorStore, buffer_bytes: usize) -> Self {
        self.descriptors = Some(descriptors);
        self.buffer_bytes = buffer_bytes;
        self
    }

    /// The plugin's 404 for an input index that does not exist. A name with
    /// a wildcard or a list is not looked up.
    fn check_indices(&self, req: &mut Request<'_>, monitor: &Value) -> Result<(), RestError> {
        let Some(descriptors) = &self.descriptors else {
            return Ok(());
        };
        let names: Vec<&str> = monitor["inputs"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|input| {
                let search = input.pointer("/search/indices");
                let doc = input.pointer("/doc_level_input/indices");
                search.into_iter().chain(doc)
            })
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(Value::as_str)
            .filter(|n| !n.contains(['*', ',']))
            .collect();
        if names.is_empty() {
            return Ok(());
        }
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one record buffer a request, buffer_bytes, reserved above.
        let mut buf = vec![0u8; self.buffer_bytes];
        let mut missing: Vec<&str> = Vec::new();
        for name in names {
            let target = descriptors
                .resolve(name, &mut buf)
                .map_err(|e| failed(&e))?;
            if matches!(target.target, oss_meta::Target::Absent) && !missing.contains(&name) {
                missing.push(name);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let reason = format!("Configured indices are not found: [{}]", missing.join(", "));
        let cause = format!(
            "org.opensearch.index.IndexNotFoundException: no such index [{}]",
            missing.first().copied().unwrap_or_default()
        );
        Err(alerting(404, reason, Some(cause)))
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
        put: (i64, i64),
        monitor: &Value,
    ) -> Result<(), RestError> {
        let body = json!({
            "_id": id,
            "_version": put.0,
            "_seq_no": put.1,
            "_primary_term": 1,
            "monitor": monitor,
        });
        resp.send(status, JSON, body.to_string().as_bytes())
    }

    fn checked(&self, req: &mut Request<'_>, replacing: bool) -> Result<Checked, RestError> {
        let bytes = read_body(req, MAX_MONITOR_BYTES, "a monitor is over its size bound")?;
        let body: Value =
            serde_json::from_slice(&bytes).map_err(|e| bad(format!("invalid body: {e}")))?;
        Checked::from_put(&body, now_millis(), self.min_interval_secs, replacing).map_err(refused)
    }

    /// Stores the monitor as `guard` allows: its version and `_seq_no`, or
    /// the refusal the plugin answers.
    fn store_monitor(
        m: &Monitors,
        id: &str,
        checked: &Checked,
        guard: PutGuard,
    ) -> Result<(i64, i64), RestError> {
        // ALLOC: the monitor's text, at most MAX_MONITOR_BYTES, checked by the put.
        let bytes = checked.stored.to_string().into_bytes();
        let outcome = m
            .put(
                id,
                &bytes,
                checked.enabled,
                &checked.schedule,
                now_millis(),
                guard,
            )
            .map_err(|e| failed(&e))?;
        match outcome {
            PutOutcome::Stored(p) => Ok((p.version, p.seq_no)),
            PutOutcome::Refused(PutRefusal::NotFound) => {
                // ALLOC: error path; one message.
                let reason = format!("Monitor with {id} is not found");
                Err(alerting(404, reason.clone(), status_cause(&reason)))
            }
            PutOutcome::Refused(PutRefusal::Conflict) => {
                // ALLOC: error path; one message.
                let reason = format!("Document version conflict putting {id} in index {INDEX}");
                Err(alerting(409, reason.clone(), status_cause(&reason)))
            }
            PutOutcome::Refused(PutRefusal::AlreadyExists) => {
                Err(refuse(409, "alerting_exception", "the monitor id is taken"))
            }
        }
    }

    /// `POST /monitors`: a new monitor under a fresh id.
    pub(crate) fn create(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let m = self.monitors(req)?;
        let checked = self.checked(req, false)?;
        self.check_indices(req, &checked.stored)?;
        let id = new_id().map_err(|e| refuse(500, "alerting_exception", e.to_string()))?;
        let put = Self::store_monitor(&m, &id, &checked, PutGuard::Absent)?;
        Self::answer(resp, 201, &id, put, &checked.stored)
    }

    /// `PUT /monitors/{id}`: replaces a monitor that exists, when its
    /// `if_seq_no` and `if_primary_term` are the ones asked for.
    pub(crate) fn update(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let (m, id) = (self.monitors(req)?, Self::id_of(req)?);
        let number = |name: &str| -> Result<Option<i64>, RestError> {
            req.param(name).map_or(Ok(None), |v| {
                v.parse::<i64>()
                    .map(Some)
                    .map_err(|_| bad(format!("[{name}] must be a number")))
            })
        };
        let (seq, term) = (number("if_seq_no")?, number("if_primary_term")?);
        // A primary term other than the one every write has is a stale one.
        let if_seq_no = match (seq, term) {
            (Some(s), Some(1) | None) => Some(s),
            (Some(_), Some(_)) => Some(i64::MIN),
            (None, _) => None,
        };
        let checked = self.checked(req, true)?;
        self.check_indices(req, &checked.stored)?;
        let put = Self::store_monitor(&m, &id, &checked, PutGuard::Exists { if_seq_no })?;
        Self::answer(resp, 200, &id, put, &checked.stored)
    }

    /// The stored monitor, or `None` for an id the tenant lacks.
    fn load(
        &self,
        req: &mut Request<'_>,
    ) -> Result<(String, Option<(oss_format::MonitorEntry, Value)>), RestError> {
        let (m, id) = (self.monitors(req)?, Self::id_of(req)?);
        req.body().reserve(MAX_MONITOR_BYTES)?;
        // ALLOC: one body buffer a request, MAX_MONITOR_BYTES, reserved above.
        let mut buf = vec![0u8; MAX_MONITOR_BYTES];
        let Some((entry, n)) = m.get(&id, &mut buf).map_err(|e| failed(&e))? else {
            return Ok((id, None));
        };
        let monitor: Value = serde_json::from_slice(buf.get(..n).unwrap_or_default())
            .map_err(|_| refuse(500, "alerting_exception", "a stored monitor is unreadable"))?;
        Ok((id, Some((entry, monitor))))
    }

    /// The stored monitor, or the plugin's 404.
    fn read(
        &self,
        req: &mut Request<'_>,
    ) -> Result<(String, oss_format::MonitorEntry, Value), RestError> {
        match self.load(req)? {
            (id, Some((entry, monitor))) => Ok((id, entry, monitor)),
            (_, None) => Err(not_found()),
        }
    }

    /// `GET /monitors/{id}`.
    pub(crate) fn get(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let (id, entry, monitor) = self.read(req)?;
        let body = json!({
            "_id": id,
            "_version": entry.version,
            "_seq_no": entry.seq_no,
            "_primary_term": 1,
            "monitor": monitor,
            "associated_workflows": [],
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// `HEAD /monitors/{id}`: 200 when the monitor exists.
    pub(crate) fn head(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        self.read(req)?;
        resp.send(200, JSON, b"")
    }

    /// `DELETE /monitors/{id}`: answers the id and the version the delete
    /// takes it to.
    pub(crate) fn delete(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let (m, id) = (self.monitors(req)?, Self::id_of(req)?);
        let Some(old) = m.delete(&id).map_err(|e| failed(&e))? else {
            // ALLOC: error path; one message.
            let reason = format!("Monitor with {id} is not found");
            return Err(RestError::exception(
                404,
                Exception::new("status_exception", reason),
            ));
        };
        let body = json!({"_id": id, "_version": old.version.saturating_add(1)});
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}
