//! `/_plugins/_anomaly_detection`: the anomaly detection plugin's detector
//! routes, served natively over a document set (ADR 0078).
//!
//! A detector is one document of the `ad/detectors/` set: a put is the
//! marker's read, the content's write and the marker's swap; a read the
//! marker's read and the content's; nothing lists. The plugin's `_seq_no` is
//! a count kept in the stored document, one per detector, so a change to
//! another detector never moves it.

mod detector;
mod job;
mod preview;
mod preview_route;
mod profile_route;
mod results_route;
mod routes;
mod run;
mod schedule;
mod search;
mod slots;
mod source;
mod validate_route;
pub mod windows;

pub use run::{MAX_HELD_MODELS, Outcome, RunError, Runner};
pub use schedule::{Scheduler, TickReport};
pub use slots::Timing;
pub use source::WindowSearch;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::BlobStore;
use oss_format::DocKind;
use oss_meta::{DocSet, MetaError};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use crate::route_util::{read_body, refuse};
pub use routes::routes;

/// The plugin's config index, shown as every detector's `_index`.
const INDEX: &str = ".opendistro-anomaly-detectors";
/// The most hits a search page returns.
const MAX_PAGE: usize = 100;

/// Serves the detector routes over one deployment's object store.
pub struct AdService {
    store: Arc<dyn BlobStore>,
    base: String,
    buffer_bytes: usize,
    search: Option<Arc<dyn WindowSearch>>,
}

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn not_found(id: &str) -> RestError {
    refuse(
        404,
        "status_exception",
        // ALLOC: error path; one message.
        format!("Can't find anomaly detector with id:{id}"),
    )
}

fn conflict(id: &str) -> RestError {
    refuse(
        409,
        "version_conflict_engine_exception",
        // ALLOC: error path; one message.
        format!("version conflict, document [{id}] was changed or already exists"),
    )
}

/// What a failed store call answers.
fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyDocs => refuse(
            429,
            "too_many_detectors",
            "a tenant holds at most 1000 detectors",
        ),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => bad(e.to_string()),
        MetaError::AlreadyExists => conflict("detector"),
        // ALLOC: error path; one message.
        other => refuse(500, "ad_store_failure", other.to_string()),
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A fresh id for a feature or a detector.
///
/// A document name may not start with `_`, so an id that does is drawn again.
fn fresh_id() -> Result<String, String> {
    loop {
        let id =
            oss_meta::new_id().map_err(|_| "the node has no source of randomness".to_owned())?;
        if !id.starts_with('_') {
            return Ok(id);
        }
    }
}

/// The plugin's body for a detector: its stored document without the count.
fn shown(id: &str, stored: &Value) -> Value {
    let seq = stored["seq_no"].as_i64().unwrap_or(0);
    json!({
        "_id": id,
        "_version": seq.saturating_add(1),
        "_seq_no": seq,
        "_primary_term": 1,
        "anomaly_detector": stored["detector"],
    })
}

impl AdService {
    /// A service over the detectors under `base` in `store`; a request holds
    /// up to `buffer_bytes` for a marker.
    pub fn new(store: &Arc<dyn BlobStore>, base: &str, buffer_bytes: usize) -> Self {
        Self {
            store: Arc::clone(store),
            // ALLOC: one copy of the base, once.
            base: base.to_owned(),
            buffer_bytes,
            search: None,
        }
    }

    fn set(&self) -> Result<DocSet, RestError> {
        DocSet::new(Arc::clone(&self.store), &self.base, DocKind::AdDetectors)
            .map_err(|e| failed(&e))
    }

    fn marker(&self, req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
        Ok(vec![0u8; self.buffer_bytes])
    }

    fn doc_buffer(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        let bytes = DocKind::AdDetectors.max_doc_bytes();
        req.body().reserve(bytes)?;
        // ALLOC: one document buffer a request, the kind's bound, reserved above.
        Ok(vec![0u8; bytes])
    }

    fn id_of(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("id")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("a detector id is required"))
    }

    fn stored(
        &self,
        id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Option<Value>, RestError> {
        match self.set()?.get(id, marker, doc) {
            Ok(Some(n)) => serde_json::from_slice(doc.get(..n).unwrap_or_default())
                .map(Some)
                .map_err(|_| refuse(500, "ad_store_failure", "a stored detector is unreadable")),
            Ok(None) => Ok(None),
            Err(e) => Err(failed(&e)),
        }
    }

    fn checked(req: &mut Request<'_>) -> Result<Value, RestError> {
        let max = DocKind::AdDetectors.max_doc_bytes();
        let raw = read_body(req, max, "the body is over the limit for a detector")?;
        let body: Value =
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?;
        detector::checked(&body, now_millis(), &mut fresh_id).map_err(bad)
    }

    fn write(
        &self,
        id: &str,
        seq: i64,
        detector: &Value,
        job: &Value,
        create: bool,
        marker: &mut [u8],
    ) -> Result<Value, RestError> {
        let stored = if job.is_null() {
            json!({"seq_no": seq, "detector": detector})
        } else {
            json!({"seq_no": seq, "detector": detector, "job": job})
        };
        // ALLOC: the document's text, at most the kind's bound, checked by the put.
        let bytes = stored.to_string().into_bytes();
        self.set()?
            .put_counted(id, &bytes, create, marker)
            .map_err(|e| failed(&e))?;
        Ok(stored)
    }

    /// `POST /detectors`: a new detector under a generated id.
    pub(crate) fn create(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let detector = Self::checked(req)?;
        let id = fresh_id().map_err(|e| refuse(500, "ad_store_failure", e))?;
        let mut marker = self.marker(req)?;
        let stored = self.write(&id, 0, &detector, &Value::Null, true, &mut marker)?;
        resp.send(201, JSON, shown(&id, &stored).to_string().as_bytes())
    }

    /// `PUT /detectors/{id}`: replaces an existing detector; with
    /// `if_seq_no` and `if_primary_term`, only as it is named.
    pub(crate) fn replace(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let number = |name: &str| -> Result<Option<i64>, RestError> {
            req.param(name)
                .map(|v| {
                    v.parse::<i64>()
                        .map_err(|_| bad(format!("[{name}] must be a number")))
                })
                .transpose()
        };
        let (if_seq, if_term) = (number("if_seq_no")?, number("if_primary_term")?);
        if if_seq.is_some() != if_term.is_some() {
            return Err(bad("[if_seq_no] and [if_primary_term] go together"));
        }
        let detector = Self::checked(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let current = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| not_found(&id))?;
        let seq = current["seq_no"].as_i64().unwrap_or(0);
        if if_seq.is_some_and(|s| s != seq) || if_term.is_some_and(|t| t != 1) {
            return Err(conflict(&id));
        }
        let stored = self.write(
            &id,
            seq.saturating_add(1),
            &detector,
            &current["job"],
            false,
            &mut marker,
        )?;
        resp.send(200, JSON, shown(&id, &stored).to_string().as_bytes())
    }

    /// `GET /detectors/{id}`.
    pub(crate) fn get(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let stored = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| not_found(&id))?;
        let mut out = shown(&id, &stored);
        if req.param("job").as_deref() == Some("true") && !stored["job"].is_null() {
            out["anomaly_detector_job"] = stored["job"].clone();
        }
        resp.send(200, JSON, out.to_string().as_bytes())
    }

    /// `DELETE /detectors/{id}`.
    pub(crate) fn delete(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let mut marker = self.marker(req)?;
        if !self
            .set()?
            .delete(&id, &mut marker)
            .map_err(|e| failed(&e))?
        {
            return Err(not_found(&id));
        }
        let body = json!({
            "_index": INDEX, "_id": id, "_version": 1, "result": "deleted",
            "forced_refresh": true,
            "_shards": {"total": 1, "successful": 1, "failed": 0},
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// `GET|POST /detectors/_count`: how many detectors the tenant holds.
    pub(crate) fn count(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let mut marker = self.marker(req)?;
        let n = self
            .set()?
            .names(&mut marker)
            .map_err(|e| failed(&e))?
            .len();
        resp.send(200, JSON, json!({"count": n}).to_string().as_bytes())
    }

    /// `GET|POST /detectors/_search`: a page of detectors in id order,
    /// chosen by a query the names can answer (see [`search`]).
    pub(crate) fn search(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let max = DocKind::AdDetectors.max_doc_bytes();
        let raw = read_body(req, max, "the body is over the limit for a search")?;
        let body: Value = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?
        };
        let number = |name: &str, default: usize| -> Result<usize, RestError> {
            match body.get(name).filter(|v| !v.is_null()) {
                Some(v) => v
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| bad(format!("[{name}] must be a whole number"))),
                None => req.param(name).map_or(Ok(default), |v| {
                    v.parse::<usize>()
                        .map_err(|_| bad(format!("[{name}] must be a number")))
                }),
            }
        };
        let (from, size) = (number("from", 0)?, number("size", 10)?.min(MAX_PAGE));
        let query = body
            .get("query")
            .cloned()
            .unwrap_or_else(|| json!({"match_all": {}}));
        let descending = match body.get("sort") {
            None => false,
            Some(sort) => {
                let first = sort.as_array().and_then(|a| a.first()).unwrap_or(sort);
                let (field, order) = match first {
                    Value::String(s) => (s.as_str(), "asc"),
                    Value::Object(m) if m.len() == 1 => {
                        let (k, v) = m.iter().next().ok_or_else(|| bad("empty sort"))?;
                        let order = v.get("order").unwrap_or(v).as_str().unwrap_or("asc");
                        (k.as_str(), order)
                    }
                    _ => return Err(bad("a sort must name one field")),
                };
                if field != "_id" {
                    return Err(bad(format!(
                        "sort on [{field}] is not served: it would read every detector; sort on [_id]"
                    )));
                }
                order == "desc"
            }
        };
        search::check(&query).map_err(bad)?;
        let mut marker = self.marker(req)?;
        let mut names = self.set()?.names(&mut marker).map_err(|e| failed(&e))?;
        let mut keep = Vec::with_capacity(names.len());
        for n in names.drain(..) {
            if search::matches(&query, &n, 0).map_err(bad)? {
                keep.push(n);
            }
        }
        if descending {
            keep.reverse();
        }
        let mut doc = Self::doc_buffer(req)?;
        // ALLOC: one entry per hit, at most the page size, 100.
        let mut hits = Vec::new();
        for name in keep.iter().skip(from).take(size) {
            if let Some(stored) = self.stored(name, &mut marker, &mut doc)? {
                let shown = shown(name, &stored);
                hits.push(json!({
                    "_index": INDEX, "_id": name, "_version": shown["_version"],
                    "_seq_no": shown["_seq_no"], "_primary_term": 1, "_score": 1.0,
                    "_source": shown["anomaly_detector"],
                }));
            }
        }
        let out = json!({
            "took": 0, "timed_out": false,
            "_shards": {"total": 1, "successful": 1, "skipped": 0, "failed": 0},
            "hits": {"total": {"value": keep.len(), "relation": "eq"}, "max_score": 1.0, "hits": hits},
        });
        resp.send(200, JSON, out.to_string().as_bytes())
    }
}
