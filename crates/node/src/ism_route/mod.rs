//! `/_plugins/_ism`: the index state management plugin's policy routes,
//! served natively over a document set (ADR 0077).
//!
//! A policy is one document of the `ism/policies/` set: a put is the
//! marker's read, the content's write and the marker's swap; a read the
//! marker's read and the content's; nothing lists. The plugin's `_seq_no`
//! is a count kept in the stored document, one per policy, so a change to
//! another policy never moves it.

#[cfg(feature = "coordinator")]
mod hook;
mod managed;
mod policy;
mod routes;
mod templates;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::BlobStore;
use oss_format::DocKind;
use oss_meta::{DescriptorStore, DocSet, MetaError};
use oss_rest::{Exception, JSON, Meta, Request, Response, RestError};
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
    descriptors: Option<DescriptorStore>,
}

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn not_found() -> RestError {
    refuse(404, "status_exception", "Policy not found")
}

/// What a delete of a missing policy says.
fn not_found_by_id(id: &str) -> RestError {
    refuse(404, "status_exception", format!("Policy {id} is not found"))
}

/// A 409 as the plugin's index answers it: the reason, and where.
fn version_conflict(reason: String) -> RestError {
    RestError::exception(
        409,
        Exception::new("version_conflict_engine_exception", reason)
            .with_meta("index", Meta::Str(INDEX.to_owned()))
            .with_meta("shard", Meta::Str("0".to_owned()))
            .with_meta("index_uuid", Meta::Str("_na_".to_owned())),
    )
}

/// A replace whose `if_seq_no` or `if_primary_term` is not the policy's.
fn conflict(id: &str, seq: i64, term: i64, current: Option<i64>) -> RestError {
    let (now_seq, now_term) = current.map_or((-2, 0), |c| (c, 1));
    version_conflict(format!(
        "[{id}]: version conflict, required seqNo [{seq}], primary term [{term}]. current document has seqNo [{now_seq}] and primary term [{now_term}]"
    ))
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
        MetaError::AlreadyExists => {
            version_conflict("version conflict, document already exists".to_owned())
        }
        // ALLOC: error path; one message.
        other => refuse(500, "ism_store_failure", other.to_string()),
    }
}

/// Whether policies may be listed sorted by `field`: they come in id order,
/// so only that field is served; a field with no mapping is the plugin's 400.
fn sortable(field: &str) -> Result<(), RestError> {
    match field {
        "policy.policy_id.keyword" | "policy.policy_id" | "_id" => Ok(()),
        "policy.description"
        | "policy.last_updated_time"
        | "policy.schema_version"
        | "policy.default_state" => Err(bad(format!(
            "sorting policies by [{field}] is not supported: they are listed in id order"
        ))),
        _ => {
            let reason = format!("No mapping found for [{field}] in order to sort on");
            let shard = || {
                Exception::new("query_shard_exception", reason.clone())
                    .with_meta("index", Meta::Str(INDEX.to_owned()))
                    .with_meta("index_uuid", Meta::Str("_na_".to_owned()))
            };
            Err(RestError::exception(
                400,
                Exception::new("search_phase_execution_exception", "all shards failed")
                    .with_meta("phase", Meta::Str("query".to_owned()))
                    .caused_by(shard()),
            ))
        }
    }
}

/// A request body as JSON: an empty one is the plugin's `parse_exception`.
fn parse_body(raw: &[u8]) -> Result<Value, RestError> {
    if raw.is_empty() {
        return Err(refuse(400, "parse_exception", "request body is required"));
    }
    serde_json::from_slice(raw).map_err(|e| bad(format!("invalid body: {e}")))
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
            descriptors: None,
        }
    }

    /// Looks indices up in `descriptors`, which `add` needs; without them it
    /// is refused 501.
    #[must_use]
    pub fn with_descriptors(mut self, descriptors: DescriptorStore) -> Self {
        self.descriptors = Some(descriptors);
        self
    }

    fn set(&self) -> Result<DocSet, RestError> {
        self.set_of(DocKind::IsmPolicies)
    }

    fn set_of(&self, kind: DocKind) -> Result<DocSet, RestError> {
        DocSet::new(Arc::clone(&self.store), &self.base, kind).map_err(|e| failed(&e))
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
        self.stored_in(DocKind::IsmPolicies, id, marker, doc)
    }

    fn stored_in(
        &self,
        kind: DocKind,
        id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Option<Value>, RestError> {
        match self.set_of(kind)?.get(id, marker, doc) {
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
        let body = parse_body(&raw)?;
        let policy = policy::checked(&id, &body, now_millis()).map_err(bad)?;
        let wanted = templates::entries_of(&id, &policy).map_err(bad)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        // A new policy with no template has nothing to remove: no read.
        let held = if wanted.is_empty() && if_seq.is_none() {
            None
        } else {
            Some(self.check_templates(&wanted, &mut marker, &mut doc)?)
        };
        let seq = match (if_seq, if_term) {
            (None, None) => 0,
            (Some(seq), Some(term)) => {
                let current = self
                    .stored(&id, &mut marker, &mut doc)?
                    .ok_or_else(|| conflict(&id, seq, term, None))?;
                let now = current["seq_no"].as_i64();
                if now != Some(seq) || term != 1 {
                    return Err(conflict(&id, seq, term, now));
                }
                seq.saturating_add(1)
            }
            _ => return Err(bad("[if_seq_no] and [if_primary_term] go together")),
        };
        let stored = json!({"seq_no": seq, "policy": policy});
        // ALLOC: the document's text, at most the kind's bound, checked by the put.
        let bytes = stored.to_string().into_bytes();
        let replaced = if_seq.is_some();
        match self.set()?.put_counted(&id, &bytes, !replaced, &mut marker) {
            Ok(_) => {}
            Err(MetaError::AlreadyExists) => {
                let now = self
                    .stored(&id, &mut marker, &mut doc)
                    .ok()
                    .flatten()
                    .and_then(|s| s["seq_no"].as_i64())
                    .unwrap_or(0);
                return Err(version_conflict(format!(
                    "[{id}]: version conflict, document already exists (current version [{}])",
                    now.saturating_add(1)
                )));
            }
            Err(e) => return Err(failed(&e)),
        }
        if let Some(held) = held {
            self.sync_templates(&id, &wanted, held, &mut marker, &mut doc)?;
        }
        // The plugin answers a put with the policy under a `policy` of its own.
        let mut answer = shown(&id, &stored);
        answer["policy"] = json!({"policy": stored["policy"]});
        resp.send(
            if replaced { 200 } else { 201 },
            JSON,
            answer.to_string().as_bytes(),
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
            return Err(not_found_by_id(&id));
        }
        let mut doc = Self::doc_buffer(req)?;
        let held = self.check_templates(&[], &mut marker, &mut doc)?;
        self.sync_templates(&id, &[], held, &mut marker, &mut doc)?;
        let body = json!({
            "_index": INDEX, "_id": id, "_version": now_millis(), "result": "deleted",
            "forced_refresh": true, "_shards": {"total": 2, "successful": 1, "failed": 0},
            "_seq_no": now_millis(), "_primary_term": 1,
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
        if let Some(field) = req.param("sortField") {
            sortable(&field)?;
        }
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
