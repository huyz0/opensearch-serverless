//! `/_plugins/_reports/definition`: the reporting plugin's report
//! definitions, served natively over a document set (see the reports spec).
//!
//! A definition is one document of the `reports/definitions/` set: a create
//! is the marker's read, the content's write and the marker's swap; a read
//! the marker's read and the content's; nothing lists the store.

mod cron;
mod definition;
mod routes;

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::BlobStore;
use oss_format::DocKind;
use oss_meta::{DocSet, MetaError, new_id};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use crate::route_util::{read_body, refuse};
pub(crate) use routes::mount;
pub use routes::routes;

/// The most definitions one list answers.
const MAX_LIST: usize = 100;

/// Serves the definition routes over one deployment's object store.
pub struct ReportsService {
    store: Arc<dyn BlobStore>,
    base: String,
    buffer_bytes: usize,
}

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn not_found(id: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(
        404,
        "status_exception",
        format!("report definition {id} not found"),
    )
}

fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyDocs => refuse(
            429,
            "report_limit",
            "a deployment holds at most 500 report definitions",
        ),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => bad(e.to_string()),
        // ALLOC: error path; one message.
        other => refuse(500, "reports_store_failure", other.to_string()),
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A new id: random, and never one a document name may not start with.
fn fresh_id() -> Result<String, RestError> {
    for _ in 0..16 {
        let id = new_id()
            .map_err(|_| refuse(500, "reports_store_failure", "no randomness for an id"))?;
        if !id.starts_with(['_', '-', '.']) {
            return Ok(id);
        }
    }
    Err(refuse(
        500,
        "reports_store_failure",
        "no usable id after 16 tries",
    ))
}

/// The plugin's answer for one stored definition.
fn details(id: &str, stored: &Value) -> Value {
    json!({
        "id": id,
        "lastUpdatedTimeMs": stored["lastUpdatedTimeMs"],
        "createdTimeMs": stored["createdTimeMs"],
        "reportDefinition": stored["reportDefinition"],
    })
}

impl ReportsService {
    /// A service over the definitions under `base` in `store`; a request
    /// holds up to `buffer_bytes` for a marker.
    pub fn new(store: &Arc<dyn BlobStore>, base: &str, buffer_bytes: usize) -> Self {
        Self {
            store: Arc::clone(store),
            // ALLOC: one copy of the base, once.
            base: base.to_owned(),
            buffer_bytes,
        }
    }

    fn set(&self) -> Result<DocSet, RestError> {
        DocSet::new(
            Arc::clone(&self.store),
            &self.base,
            DocKind::ReportDefinitions,
        )
        .map_err(|e| failed(&e))
    }

    fn marker(&self, req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
        Ok(vec![0u8; self.buffer_bytes])
    }

    fn doc_buffer(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        let bytes = DocKind::ReportDefinitions.max_doc_bytes();
        req.body().reserve(bytes)?;
        // ALLOC: one document buffer a request, the kind's bound, reserved above.
        Ok(vec![0u8; bytes])
    }

    fn id(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("id")
            .map(Cow::into_owned)
            .ok_or_else(|| bad("a definition id is required"))
    }

    fn read(
        set: &DocSet,
        id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Option<Value>, RestError> {
        let Some(n) = set.get(id, marker, doc).map_err(|e| failed(&e))? else {
            return Ok(None);
        };
        serde_json::from_slice(doc.get(..n).unwrap_or_default())
            .map(Some)
            .map_err(|_| {
                refuse(
                    500,
                    "reports_store_failure",
                    "a stored definition is unreadable",
                )
            })
    }

    fn body(req: &mut Request<'_>) -> Result<Value, RestError> {
        let max = DocKind::ReportDefinitions.max_doc_bytes();
        let raw = read_body(req, max, "the body is over the limit for a definition")?;
        if raw.iter().all(u8::is_ascii_whitespace) {
            return Err(refuse(400, "parse_exception", "request body is required"));
        }
        let body: Value = serde_json::from_slice(&raw)
            .map_err(|e| refuse(400, "parse_exception", format!("invalid body: {e}")))?;
        definition::check(&body)?;
        Ok(body)
    }

    fn stored(body: Value, created: i64, updated: i64) -> Vec<u8> {
        // ALLOC: the document's text, bounded by the body's, checked by the put.
        json!({"createdTimeMs": created, "lastUpdatedTimeMs": updated, "reportDefinition": body})
            .to_string()
            .into_bytes()
    }

    /// `POST /definition`: stores the definition under a new id.
    pub(crate) fn create(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let body = Self::body(req)?;
        let id = fresh_id()?;
        let now = now_millis();
        let bytes = Self::stored(body, now, now);
        let mut marker = self.marker(req)?;
        self.set()?
            .put_counted(&id, &bytes, true, &mut marker)
            .map_err(|e| failed(&e))?;
        resp.send(
            200,
            JSON,
            json!({"reportDefinitionId": id}).to_string().as_bytes(),
        )
    }

    /// `PUT /definition/{id}`: replaces a definition, keeping its creation time.
    pub(crate) fn replace(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id(req)?;
        let body = Self::body(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let set = self.set()?;
        let old = Self::read(&set, &id, &mut marker, &mut doc)?.ok_or_else(|| not_found(&id))?;
        let bytes = Self::stored(
            body,
            old["createdTimeMs"].as_i64().unwrap_or(0),
            now_millis(),
        );
        set.put_counted(&id, &bytes, false, &mut marker)
            .map_err(|e| failed(&e))?;
        resp.send(
            200,
            JSON,
            json!({"reportDefinitionId": id}).to_string().as_bytes(),
        )
    }

    /// `GET /definition/{id}`.
    pub(crate) fn get(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let stored =
            Self::read(&self.set()?, &id, &mut marker, &mut doc)?.ok_or_else(|| not_found(&id))?;
        let body = json!({"reportDefinitionDetails": details(&id, &stored)});
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// `DELETE /definition/{id}`.
    pub(crate) fn delete(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id(req)?;
        let mut marker = self.marker(req)?;
        if !self
            .set()?
            .delete(&id, &mut marker)
            .map_err(|e| failed(&e))?
        {
            return Err(not_found(&id));
        }
        resp.send(
            200,
            JSON,
            json!({"reportDefinitionId": id}).to_string().as_bytes(),
        )
    }

    /// `GET /definitions`: newest first, paged by `fromIndex` and `maxItems`.
    pub(crate) fn list(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let number = |name: &str, default: usize| -> Result<usize, RestError> {
            match req.param(name) {
                None => Ok(default),
                Some(v) => v.parse().map_err(|_| {
                    bad(format!(
                        "[{name}] must be a non-negative integer, not [{v}]"
                    ))
                }),
            }
        };
        let from = number("fromIndex", 0)?;
        let max = number("maxItems", MAX_LIST)?.min(MAX_LIST);
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let set = self.set()?;
        let mut all: Vec<(i64, String, Value)> = Vec::new();
        set.for_each(&mut marker, &mut doc, &mut |id, bytes| {
            if let Ok(stored) = serde_json::from_slice::<Value>(bytes) {
                // ALLOC: one entry each, at most the kind's bound of definitions.
                all.push((
                    stored["lastUpdatedTimeMs"].as_i64().unwrap_or(0),
                    id.to_owned(),
                    stored,
                ));
            }
        })
        .map_err(|e| failed(&e))?;
        all.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let total = all.len();
        let page: Vec<Value> = all
            .iter()
            .skip(from)
            .take(max)
            .map(|(_, id, s)| details(id, s))
            .collect();
        let body = json!({"totalHits": total, "reportDefinitionDetailsList": page});
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}
