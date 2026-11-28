//! `/_plugins/_ism`: the index state management plugin's policy routes,
//! served natively over a document set (ADR 0077).
//!
//! A policy is one document of the `ism/policies/` set: a put is the
//! marker's read, the content's write and the marker's swap; a read the
//! marker's read and the content's; nothing lists. The plugin's `_seq_no`
//! is a count kept in the stored document, one per policy, so a change to
//! another policy never moves it.

mod policy;
mod routes;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::BlobStore;
use oss_format::DocKind;
use oss_meta::{DocSet, MetaError};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use crate::route_util::{read_body, refuse};
pub use routes::routes;

/// The plugin's config index, shown as every policy's `_index`.
const INDEX: &str = ".opendistro-ism-config";

/// Serves the policy routes over one deployment's object store.
pub struct IsmService {
    store: Arc<dyn BlobStore>,
    base: String,
    buffer_bytes: usize,
}

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn not_found() -> RestError {
    refuse(404, "status_exception", "Policy not found")
}

fn conflict(id: &str) -> RestError {
    refuse(
        409,
        "version_conflict_engine_exception",
        format!("version conflict, document [{id}] was changed or already exists"),
    )
}

/// What a failed store call answers.
fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyDocs => refuse(
            429,
            "too_many_policies",
            "a tenant holds at most 1000 policies",
        ),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => bad(e.to_string()),
        MetaError::AlreadyExists => refuse(
            409,
            "version_conflict_engine_exception",
            "a policy of that name already exists",
        ),
        // ALLOC: error path; one message.
        other => refuse(500, "ism_store_failure", other.to_string()),
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The plugin's body for a policy: its stored document without the count.
fn shown(id: &str, stored: &Value) -> Value {
    let seq = stored["seq_no"].as_i64().unwrap_or(0);
    json!({
        "_id": id,
        "_version": seq.saturating_add(1),
        "_seq_no": seq,
        "_primary_term": 1,
        "policy": stored["policy"],
    })
}

impl IsmService {
    /// A service over the policies under `base` in `store`; a request holds
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
        DocSet::new(Arc::clone(&self.store), &self.base, DocKind::IsmPolicies)
            .map_err(|e| failed(&e))
    }

    fn marker(&self, req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
        Ok(vec![0u8; self.buffer_bytes])
    }

    fn doc_buffer(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        let bytes = DocKind::IsmPolicies.max_doc_bytes();
        req.body().reserve(bytes)?;
        // ALLOC: one document buffer a request, the kind's bound, reserved above.
        Ok(vec![0u8; bytes])
    }

    fn id_of(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("id")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("a policy id is required"))
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
                .map_err(|_| refuse(500, "ism_store_failure", "a stored policy is unreadable")),
            Ok(None) => Ok(None),
            Err(e) => Err(failed(&e)),
        }
    }

    /// `PUT /policies/{id}`: a create, or a replace when `if_seq_no` and
    /// `if_primary_term` name the policy as it is.
    pub(crate) fn put(
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
        let max = DocKind::IsmPolicies.max_doc_bytes();
        let raw = read_body(req, max, "the body is over the limit for a policy")?;
        let body: Value =
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?;
        let policy = policy::checked(&id, &body, now_millis()).map_err(bad)?;
        let mut marker = self.marker(req)?;
        let seq = match (if_seq, if_term) {
            (None, None) => 0,
            (Some(seq), Some(term)) => {
                let mut doc = Self::doc_buffer(req)?;
                let current = self
                    .stored(&id, &mut marker, &mut doc)?
                    .ok_or_else(|| conflict(&id))?;
                if current["seq_no"].as_i64() != Some(seq) || term != 1 {
                    return Err(conflict(&id));
                }
                seq.saturating_add(1)
            }
            _ => return Err(bad("[if_seq_no] and [if_primary_term] go together")),
        };
        let stored = json!({"seq_no": seq, "policy": policy});
        // ALLOC: the document's text, at most the kind's bound, checked by the put.
        let bytes = stored.to_string().into_bytes();
        let replaced = if_seq.is_some();
        self.set()?
            .put_counted(&id, &bytes, !replaced, &mut marker)
            .map_err(|e| failed(&e))?;
        resp.send(
            if replaced { 200 } else { 201 },
            JSON,
            shown(&id, &stored).to_string().as_bytes(),
        )
    }

    /// `GET /policies/{id}`.
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
            .ok_or_else(not_found)?;
        resp.send(200, JSON, shown(&id, &stored).to_string().as_bytes())
    }

    /// `DELETE /policies/{id}`.
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
            return Err(not_found());
        }
        let body = json!({
            "_index": INDEX, "_id": id, "result": "deleted", "forced_refresh": true,
            "_shards": {"total": 1, "successful": 1, "failed": 0},
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// `GET /policies`: a page in id order; `queryString` keeps the ids that
    /// contain it, `sortOrder` is `asc` (the default) or `desc`.
    pub(crate) fn list(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let number = |name: &str, default: usize| -> Result<usize, RestError> {
            req.param(name).map_or(Ok(default), |v| {
                v.parse::<usize>()
                    .map_err(|_| bad(format!("[{name}] must be a number")))
            })
        };
        let (from, size) = (number("from", 0)?, number("size", 20)?.min(100));
        let contains = req
            .param("queryString")
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default()
            .replace('*', "");
        let descending = req.param("sortOrder").is_some_and(|v| v == "desc");
        let mut marker = self.marker(req)?;
        let mut names = self.set()?.names(&mut marker).map_err(|e| failed(&e))?;
        names.retain(|n| n.contains(&contains));
        if descending {
            names.reverse();
        }
        let mut doc = Self::doc_buffer(req)?;
        // ALLOC: one entry per hit, at most the page size, 100.
        let mut policies = Vec::new();
        for name in names.iter().skip(from).take(size) {
            if let Some(stored) = self.stored(name, &mut marker, &mut doc)? {
                let mut entry = shown(name, &stored);
                if let Some(m) = entry.as_object_mut() {
                    m.remove("_version");
                }
                policies.push(entry);
            }
        }
        let body = json!({"policies": policies, "total_policies": names.len()});
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}
