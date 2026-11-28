//! `/_plugins/_observability/object`: the observability plugin's object
//! store, served natively over a document set (see the observability spec).
//!
//! An object is one document of the `observability/objects/` set: a create
//! is the marker's read, the content's write and the marker's swap; a read
//! the marker's read and the content's; nothing lists. The plugin's
//! `.opensearch-observability` index is not created.

mod list;
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
pub(crate) use routes::mount;
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

fn status(code: u16, reason: impl Into<String>) -> RestError {
    refuse(code, "status_exception", reason)
}

fn not_found(id: &str) -> RestError {
    // ALLOC: error path; one message.
    status(404, format!("ObservabilityObject {id} not found"))
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

/// A new object id: random, and never one that starts with a character a
/// document name may not (`_`, `-` or `.`), which one id in 32 would.
fn fresh_id() -> Result<String, RestError> {
    for _ in 0..16 {
        let id = new_id().map_err(|_| {
            refuse(
                500,
                "observability_store_failure",
                "no randomness for an id",
            )
        })?;
        if !id.starts_with(['_', '-', '.']) {
            return Ok(id);
        }
    }
    Err(refuse(
        500,
        "observability_store_failure",
        "no usable id after 16 tries",
    ))
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

    fn doc_buffer(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        let bytes = DocKind::ObservabilityObjects.max_doc_bytes();
        req.body().reserve(bytes)?;
        // ALLOC: one document buffer a request, the kind's bound, reserved above.
        Ok(vec![0u8; bytes])
    }

    /// The stored document `id`, parsed, or `None`.
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
                    "observability_store_failure",
                    "a stored object is unreadable",
                )
            })
    }

    fn body(req: &mut Request<'_>) -> Result<object::Parsed, RestError> {
        let max = DocKind::ObservabilityObjects.max_doc_bytes();
        let raw = read_body(req, max, "the body is over the limit for an object")?;
        object::parse(&raw)
    }

    /// `POST /object`: stores the body's object under the id it names or a
    /// new one, and answers `{"objectId": id}`.
    pub(crate) fn create(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let parsed = Self::body(req)?;
        let id = match parsed.object_id.clone() {
            Some(id) => id,
            None => fresh_id()?,
        };
        let now = now_millis();
        let bytes = stored(parsed, now, now, 1);
        let mut marker = self.marker(req)?;
        let set = self.set()?;
        match set.put_counted(&id, &bytes, true, &mut marker) {
            Ok(_) => {}
            Err(MetaError::AlreadyExists) => {
                let mut doc = Self::doc_buffer(req)?;
                let version = Self::read(&set, &id, &mut marker, &mut doc)?
                    .and_then(|d| d["version"].as_i64())
                    .unwrap_or(1);
                return Err(status(
                    409,
                    // ALLOC: error path; one message.
                    format!(
                        "[{id}]: version conflict, document already exists (current version [{version}])"
                    ),
                ));
            }
            Err(e) => return Err(failed(&e)),
        }
        resp.send(200, JSON, json!({"objectId": id}).to_string().as_bytes())
    }

    /// `PUT /object/{id}`: replaces the object, keeping its type and its
    /// creation time.
    pub(crate) fn replace(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id(req)?;
        let parsed = Self::body(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let set = self.set()?;
        let old = Self::read(&set, &id, &mut marker, &mut doc)?.ok_or_else(|| not_found(&id))?;
        if object::type_of(&old).is_none_or(|(ty, _)| ty != parsed.ty) {
            return Err(status(409, "Object type cannot be changed after creation"));
        }
        let created = old["createdTimeMs"].as_i64().unwrap_or(0);
        let version = old["version"].as_i64().unwrap_or(1).saturating_add(1);
        let bytes = stored(parsed, created, now_millis(), version);
        set.put_counted(&id, &bytes, false, &mut marker)
            .map_err(|e| failed(&e))?;
        resp.send(200, JSON, json!({"objectId": id}).to_string().as_bytes())
    }

    /// `GET /object/{id}`: the plugin's list answer with the one object.
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
        let body = json!({
            "startIndex": 0,
            "totalHits": 1,
            "totalHitRelation": "eq",
            "observabilityObjectList": [list::shape(&id, &stored)],
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// `DELETE /object/{id}` and `DELETE /object?objectIdList=a,b`: all of
    /// the named objects or, when one is missing, none.
    pub(crate) fn delete(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let single = req.path_param("id").map(std::borrow::Cow::into_owned);
        let ids: Vec<String> = match (&single, req.param("objectId"), req.param("objectIdList")) {
            (Some(id), _, _) => vec![id.clone()],
            (None, Some(id), _) => vec![id.into_owned()],
            (None, None, Some(list)) => list::split(&list),
            (None, None, None) => {
                return resp.send(
                    400,
                    JSON,
                    b"\"either objectId or objectIdList is required\"",
                );
            }
        };
        if ids.len() > MAX_IDS {
            return Err(bad(format!("at most {MAX_IDS} object ids a request")));
        }
        let mut marker = self.marker(req)?;
        let set = self.set()?;
        let present = set.names(&mut marker).map_err(|e| failed(&e))?;
        let missing: Vec<&str> = ids
            .iter()
            .filter(|i| {
                present
                    .binary_search_by(|p| p.as_str().cmp(i.as_str()))
                    .is_err()
            })
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            let named = if single.is_some() || ids.len() == 1 {
                missing.join(", ")
            } else {
                format!("[{}]", missing.join(", "))
            };
            return Err(status(
                404,
                format!("ObservabilityObject {named} not found"),
            ));
        }
        let mut done = serde_json::Map::new();
        for id in &ids {
            set.delete(id, &mut marker).map_err(|e| failed(&e))?;
            done.insert(id.clone(), json!("OK"));
        }
        resp.send(
            200,
            JSON,
            json!({"deleteResponseList": done}).to_string().as_bytes(),
        )
    }

    fn id(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("id")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("an object id is required"))
    }
}

/// The most object ids one request may name.
const MAX_IDS: usize = 100;

/// The stored form of an object: its bookkeeping and the body under its type.
fn stored(parsed: object::Parsed, created: i64, updated: i64, version: i64) -> Vec<u8> {
    let mut doc = serde_json::Map::new();
    doc.insert("createdTimeMs".to_owned(), json!(created));
    doc.insert("lastUpdatedTimeMs".to_owned(), json!(updated));
    doc.insert("version".to_owned(), json!(version));
    doc.insert(parsed.ty.to_owned(), Value::Object(parsed.object));
    // ALLOC: the document's text, bounded by the body's, checked by the put.
    Value::Object(doc).to_string().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_never_starts_with_a_character_a_name_may_not() {
        for _ in 0..5_000 {
            let id = fresh_id().expect("an id");
            assert!(!id.starts_with(['_', '-', '.']), "{id}");
            assert_eq!(id.len(), 22);
        }
    }
}
