//! `/_plugins/_ml`: the ML Commons plugin's model registry, served natively
//! over document sets (ADR 0079).
//!
//! A model group is one document of the `ml/model_groups/` set, named by its
//! generated id. Its name is unique in its access scope, and no LIST can find
//! a duplicate, so the name is claimed by a blob of its own, made by a
//! conditional create, that holds the id: a register is the claim's create, the
//! marker's read, the content's write and the marker's swap (1 GET and 3 PUTs);
//! a read the marker's read and the content's; nothing lists.

mod routes;
mod search;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::{BlobError, BlobStore, ByteRange, CreateOutcome, SliceSource};
use oss_format::DocKind;
use oss_meta::{DocSet, MetaError, new_id};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Map, Value, json};

use crate::route_util::{read_body, refuse};
pub use routes::routes;

/// The plugin's index, shown as every model group's `_index`.
pub(crate) const INDEX: &str = ".plugins-ml-model-group";
/// The longest model group name, as the longest document name.
const MAX_NAME_BYTES: usize = 255;
/// A claim holds one id, 22 bytes; a buffer a little larger reads it whole.
const CLAIM_BUFFER: usize = 64;
/// How old a claim must be before it can be taken over from a group that was
/// never written: far longer than a register takes, so a live one is never robbed.
const STALE_CLAIM_MILLIS: u64 = 120_000;

/// Serves the registry routes over one deployment's object store.
pub struct MlService {
    store: Arc<dyn BlobStore>,
    base: String,
    buffer_bytes: usize,
    stale_millis: u64,
}

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn missing(id: &str) -> RestError {
    refuse(
        404,
        "status_exception",
        // ALLOC: error path; one message.
        format!("Failed to find model group with the provided model group id: {id}"),
    )
}

fn duplicate(id: &str) -> RestError {
    bad(format!(
        "The name you provided is already being used by a model group with ID: {id}."
    ))
}

fn unreadable() -> RestError {
    refuse(
        500,
        "ml_store_failure",
        "a stored model group is unreadable",
    )
}

/// What a failed store call answers.
fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyDocs => refuse(
            429,
            "too_many_model_groups",
            "a tenant holds at most 1000 model groups",
        ),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => bad(e.to_string()),
        // ALLOC: error path; one message.
        other => refuse(500, "ml_store_failure", other.to_string()),
    }
}

fn blob_failed(e: &BlobError) -> RestError {
    // ALLOC: error path; one message.
    refuse(500, "ml_store_failure", e.to_string())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A name as lower-case hex: a claim's key segment, safe for any name.
fn hex(name: &str) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    // ALLOC: two symbols a byte of a name of at most MAX_NAME_BYTES.
    let mut out = String::with_capacity(name.len().saturating_mul(2));
    for b in name.bytes() {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    out
}

/// A fresh id; one that starts with `_` is not a document name, so it is
/// drawn again (one in 64 is).
fn fresh_id() -> Result<String, RestError> {
    loop {
        let id = new_id().map_err(|e| blob_failed(&e))?;
        if !id.starts_with('_') {
            return Ok(id);
        }
    }
}

/// A group's name from a body: present, a string, not blank, bounded.
fn checked_name(value: &Value) -> Result<&str, RestError> {
    let name = value
        .as_str()
        .filter(|n| !n.trim().is_empty())
        .ok_or_else(|| bad("model group name is null"))?;
    if name.len() > MAX_NAME_BYTES {
        return Err(bad("model group name is too long"));
    }
    Ok(name)
}

impl MlService {
    /// A service over the registry under `base` in `store`; a request holds
    /// up to `buffer_bytes` for a marker.
    pub fn new(store: &Arc<dyn BlobStore>, base: &str, buffer_bytes: usize) -> Self {
        Self {
            store: Arc::clone(store),
            // ALLOC: one copy of the base, once.
            base: base.to_owned(),
            buffer_bytes,
            stale_millis: STALE_CLAIM_MILLIS,
        }
    }

    /// As [`MlService::new`], with the age a claim must reach before a register may take
    /// it over from a group that never came to be.
    #[must_use]
    pub fn with_stale_claim_millis(mut self, millis: u64) -> Self {
        self.stale_millis = millis;
        self
    }

    fn set(&self) -> Result<DocSet, RestError> {
        DocSet::new(Arc::clone(&self.store), &self.base, DocKind::MlModelGroups)
            .map_err(|e| failed(&e))
    }

    fn marker(&self, req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
        Ok(vec![0u8; self.buffer_bytes])
    }

    fn doc_buffer(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        let bytes = DocKind::MlModelGroups.max_doc_bytes();
        req.body().reserve(bytes)?;
        // ALLOC: one document buffer a request, the kind's bound, reserved above.
        Ok(vec![0u8; bytes])
    }

    fn id_of(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("model_group_id")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("a model group id is required"))
    }

    fn body(req: &mut Request<'_>) -> Result<Map<String, Value>, RestError> {
        let max = DocKind::MlModelGroups.max_doc_bytes();
        let raw = read_body(req, max, "the body is over the limit for a model group")?;
        match serde_json::from_slice::<Value>(&raw) {
            Ok(Value::Object(map)) => Ok(map),
            Ok(_) => Err(bad("the body must be a JSON object")),
            Err(e) => Err(bad(format!("invalid body: {e}"))),
        }
    }

    /// A body that may be absent: an empty one is an empty object.
    fn body_or_empty(req: &mut Request<'_>) -> Result<Map<String, Value>, RestError> {
        let max = DocKind::MlModelGroups.max_doc_bytes();
        let raw = read_body(req, max, "the body is over the limit for a search")?;
        if raw.iter().all(u8::is_ascii_whitespace) {
            return Ok(Map::new());
        }
        match serde_json::from_slice::<Value>(&raw) {
            Ok(Value::Object(map)) => Ok(map),
            Ok(_) => Err(bad("the body must be a JSON object")),
            Err(e) => Err(bad(format!("invalid body: {e}"))),
        }
    }

    fn claim_key(&self, name: &str) -> String {
        // ALLOC: one key a claim.
        format!("{}ml/model_group_names/public/{}", self.base, hex(name))
    }

    /// Makes the claim on `name` for `id`; the id holding it when another does.
    ///
    /// A claim that names no group of that name, and has outlived
    /// `stale_millis`, is the leftover of a register that died between its
    /// claim and its write: it is released (only if it still holds the id read)
    /// and made again once.
    fn claim(
        &self,
        name: &str,
        id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Option<String>, RestError> {
        let key = self.claim_key(name);
        let mut buf = [0u8; CLAIM_BUFFER];
        for _ in 0..2 {
            if matches!(
                self.store
                    .create_if_absent(&key, &mut SliceSource::new(id.as_bytes()), &mut buf)
                    .map_err(|e| blob_failed(&e))?,
                CreateOutcome::Created
            ) {
                return Ok(None);
            }
            let held = self.holder(name)?;
            if !self.is_stale(&key, name, held.as_deref(), marker, doc)? {
                return Ok(Some(held.unwrap_or_default()));
            }
            self.release(name, held.as_deref());
        }
        let held = self.holder(name)?;
        Ok(Some(held.unwrap_or_default()))
    }

    /// Whether the claim at `key` is a leftover: old, and its holder is not a
    /// group of this name.
    fn is_stale(
        &self,
        key: &str,
        name: &str,
        held: Option<&str>,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<bool, RestError> {
        let old = self
            .store
            .age_millis(key)
            .map_err(|e| blob_failed(&e))?
            .is_some_and(|age| age >= self.stale_millis);
        if !old {
            return Ok(false);
        }
        let Some(held) = held else {
            return Ok(true);
        };
        let group = self.stored(held, marker, doc)?;
        Ok(group.is_none_or(|g| g.get("name").and_then(Value::as_str) != Some(name)))
    }

    /// The id holding the claim on `name`, if one does (1 GET).
    pub(crate) fn holder(&self, name: &str) -> Result<Option<String>, RestError> {
        let mut bytes: Vec<u8> = Vec::new();
        let mut buf = [0u8; CLAIM_BUFFER];
        let mut sink = |chunk: &[u8]| {
            if bytes.len().saturating_add(chunk.len()) <= CLAIM_BUFFER {
                bytes.extend_from_slice(chunk);
            }
            Ok(())
        };
        match self
            .store
            .get(&self.claim_key(name), ByteRange::Full, &mut buf, &mut sink)
        {
            Ok(_) => Ok(String::from_utf8(bytes).ok().filter(|b| !b.is_empty())),
            Err(BlobError::NotFound) => Ok(None),
            Err(e) => Err(blob_failed(&e)),
        }
    }

    /// Deletes the claim on `name` if it still holds `id` (`None` for a claim
    /// that holds nothing readable), so it never removes another group's claim.
    fn release(&self, name: &str, id: Option<&str>) {
        if self.holder(name).ok().is_some_and(|h| h.as_deref() == id) {
            let _ = self.store.delete(&self.claim_key(name));
        }
    }

    pub(crate) fn stored(
        &self,
        id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Option<Map<String, Value>>, RestError> {
        match self.set()?.get(id, marker, doc) {
            Ok(Some(n)) => match serde_json::from_slice(doc.get(..n).unwrap_or_default()) {
                Ok(Value::Object(map)) => Ok(Some(map)),
                _ => Err(unreadable()),
            },
            Ok(None) => Ok(None),
            Err(e) => Err(failed(&e)),
        }
    }

    fn write(
        &self,
        id: &str,
        group: &Map<String, Value>,
        create: bool,
        marker: &mut [u8],
    ) -> Result<(), RestError> {
        // ALLOC: the document's text, at most the kind's bound, checked by the put.
        let bytes = Value::Object(group.clone()).to_string().into_bytes();
        self.set()?
            .put_counted(id, &bytes, create, marker)
            .map(|_| ())
            .map_err(|e| failed(&e))
    }

    /// `POST /model_groups/_register`.
    pub(crate) fn register_group(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let body = Self::body(req)?;
        let name = checked_name(body.get("name").unwrap_or(&Value::Null))?.to_owned();
        let description = match body.get("description") {
            None | Some(Value::Null) => None,
            Some(Value::String(d)) => Some(d.clone()),
            Some(_) => return Err(bad("[description] must be a string")),
        };
        let mut marker = self.marker(req)?;
        let id = fresh_id()?;
        let mut doc = Self::doc_buffer(req)?;
        if let Some(held) = self.claim(&name, &id, &mut marker, &mut doc)? {
            return Err(duplicate(&held));
        }
        let t = now_millis();
        let mut group = Map::new();
        group.insert("name".into(), Value::String(name.clone()));
        if let Some(d) = description {
            group.insert("description".into(), Value::String(d));
        }
        group.insert("latest_version".into(), json!(0));
        group.insert("access".into(), json!("public"));
        group.insert("created_time".into(), json!(t));
        group.insert("last_updated_time".into(), json!(t));
        if let Err(e) = self.write(&id, &group, true, &mut marker) {
            self.release(&name, Some(&id));
            return Err(e);
        }
        let out = json!({"model_group_id": id, "status": "CREATED"});
        resp.send(200, JSON, out.to_string().as_bytes())
    }

    /// `GET /model_groups/{id}`.
    pub(crate) fn get_group(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let group = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| missing(&id))?;
        resp.send(200, JSON, Value::Object(group).to_string().as_bytes())
    }

    /// `PUT /model_groups/{id}`: name and description.
    pub(crate) fn update_group(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let body = Self::body(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let mut group = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| missing(&id))?;
        let old = group
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut renamed = None;
        if let Some(value) = body.get("name").filter(|v| !v.is_null()) {
            let name = checked_name(value)?.to_owned();
            if name != old {
                if let Some(held) = self.claim(&name, &id, &mut marker, &mut doc)?
                    && held != id
                {
                    return Err(duplicate(&held));
                }
                group.insert("name".into(), Value::String(name.clone()));
                renamed = Some(name);
            }
        }
        match body.get("description") {
            None | Some(Value::Null) => {}
            Some(Value::String(d)) => {
                group.insert("description".into(), Value::String(d.clone()));
            }
            Some(_) => return Err(bad("[description] must be a string")),
        }
        group.insert("last_updated_time".into(), json!(now_millis()));
        if let Err(e) = self.write(&id, &group, false, &mut marker) {
            if let Some(name) = &renamed {
                self.release(name, Some(&id));
            }
            return Err(e);
        }
        if renamed.is_some() {
            self.release(&old, Some(&id));
        }
        resp.send(200, JSON, br#"{"status":"Updated"}"#)
    }

    /// `DELETE /model_groups/{id}`.
    pub(crate) fn delete_group(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let group = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| missing(&id))?;
        if !self
            .set()?
            .delete(&id, &mut marker)
            .map_err(|e| failed(&e))?
        {
            return Err(missing(&id));
        }
        if let Some(name) = group.get("name").and_then(Value::as_str) {
            self.release(name, Some(&id));
        }
        let out = json!({
            "_index": INDEX, "_id": id, "_version": 1, "result": "deleted",
            "forced_refresh": true,
            "_shards": {"total": 1, "successful": 1, "failed": 0},
            "_seq_no": 0, "_primary_term": 1,
        });
        resp.send(200, JSON, out.to_string().as_bytes())
    }
}
