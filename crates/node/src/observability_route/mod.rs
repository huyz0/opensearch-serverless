//! `/_plugins/_observability/object`: the observability plugin's object
//! store, served natively over a document set (see the observability spec).
//!
//! An object is one document of the `observability/objects/` set: a create
//! is the marker's read, the content's write and the marker's swap; a read
//! the marker's read and the content's; nothing lists. The plugin's
//! `.opensearch-observability` index is not created.

mod object;
mod routes;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::BlobStore;
use oss_format::DocKind;
use oss_meta::{DocSet, MetaError, new_id};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use crate::route_util::{read_body, refuse};
pub use routes::routes;

/// Serves the object routes over one deployment's object store.
pub struct ObservabilityService {
    store: Arc<dyn BlobStore>,
    base: String,
    buffer_bytes: usize,
}

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn not_found(id: &str) -> RestError {
    refuse(
        404,
        "resource_not_found_exception",
        // ALLOC: error path; one message.
        format!("observability object [{id}] not found"),
    )
}

/// What a failed store call answers.
fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyDocs => refuse(
            429,
            "object_limit",
            "a tenant holds at most 1000 observability objects",
        ),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => bad(e.to_string()),
        // ALLOC: error path; one message.
        other => refuse(500, "observability_store_failure", other.to_string()),
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl ObservabilityService {
    /// A service over the objects under `base` in `store`; a request holds
    /// up to `buffer_bytes` for a marker.
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
            DocKind::ObservabilityObjects,
        )
        .map_err(|e| failed(&e))
    }

    fn marker(&self, req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
        Ok(vec![0u8; self.buffer_bytes])
    }

    /// `POST /object`: stores the body's one object under a new id and
    /// answers `{"objectId": id}`.
    pub(crate) fn create(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let max = DocKind::ObservabilityObjects.max_doc_bytes();
        let raw = read_body(req, max, "the body is over the limit for an object")?;
        let body: Value =
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?;
        let ty = object::checked(&body).map_err(bad)?;
        let id = new_id().map_err(|_| {
            refuse(
                500,
                "observability_store_failure",
                "no randomness for an id",
            )
        })?;
        let now = now_millis();
        let stored = json!({
            "createdTimeMs": now,
            "lastUpdatedTimeMs": now,
            "tenant": "",
            "access": [],
            ty: body[ty],
        });
        // ALLOC: the document's text, bounded by the body's, checked by the put.
        let bytes = stored.to_string().into_bytes();
        let mut marker = self.marker(req)?;
        self.set()?
            .put_counted(&id, &bytes, true, &mut marker)
            .map_err(|e| failed(&e))?;
        resp.send(200, JSON, json!({"objectId": id}).to_string().as_bytes())
    }

    /// `GET /object/{id}`: the plugin's list answer with the one object.
    pub(crate) fn get(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = req
            .path_param("id")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("an object id is required"))?;
        let mut marker = self.marker(req)?;
        let bytes = DocKind::ObservabilityObjects.max_doc_bytes();
        req.body().reserve(bytes)?;
        // ALLOC: one document buffer a request, the kind's bound, reserved above.
        let mut doc = vec![0u8; bytes];
        let n = self
            .set()?
            .get(&id, &mut marker, &mut doc)
            .map_err(|e| failed(&e))?
            .ok_or_else(|| not_found(&id))?;
        let mut object: Value =
            serde_json::from_slice(doc.get(..n).unwrap_or_default()).map_err(|_| {
                refuse(
                    500,
                    "observability_store_failure",
                    "a stored object is unreadable",
                )
            })?;
        if let Some(m) = object.as_object_mut() {
            m.insert("objectId".to_owned(), Value::String(id));
        }
        let body = json!({
            "startIndex": 0,
            "totalHits": 1,
            "totalHitRelation": "eq",
            "observabilityObjectList": [object],
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}
