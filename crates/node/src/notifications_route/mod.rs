//! `/_plugins/_notifications`: the notifications plugin's channel configs,
//! served natively over two document sets (ADR 0081).
//!
//! A config is one document of the `notifications/configs/` set. A
//! **catalogue**, one document of `notifications/catalogue/`, holds every
//! config's non-secret summary, so a list reads it and not each config.
//! A create or replace writes the config and then its summary; a delete
//! drops the summary and then the config, so a crash between the two leaves
//! a config the catalogue lacks and never a summary with no config. A list
//! compares the config marker's names with the catalogue's ids and repairs a
//! difference. Nothing lists the store.

mod config;
mod deliver;
mod egress;
mod list;
mod routes;
mod test_send;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::BlobStore;
use oss_format::DocKind;
use oss_meta::{DocSet, MetaError};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Map, Value, json};

use crate::route_util::{read_body, refuse};
pub use deliver::{Deliverer, Delivery, Limits};
pub(crate) use egress::GuardedResolver;
pub use egress::{EgressPolicy, Lookup, permitted, system_lookup};
pub use routes::routes;

/// The catalogue's one document name.
const CATALOGUE: &str = "catalogue";
/// The biggest body a config request carries.
const MAX_CONFIG_BYTES: usize = 64 << 10;

/// Serves the config routes over one deployment's object store.
pub struct NotificationsService {
    store: Arc<dyn BlobStore>,
    base: String,
    buffer_bytes: usize,
    allowed: Vec<&'static str>,
    deliverer: Option<std::sync::Arc<Deliverer>>,
}

pub(crate) fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn not_found(id: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(
        404,
        "status_exception",
        format!("NotificationConfig [{id}] not found"),
    )
}

/// What a failed store call answers.
fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyDocs => refuse(
            429,
            "too_many_configs",
            "a tenant holds at most 1000 notification configs",
        ),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => bad(e.to_string()),
        MetaError::AlreadyExists => refuse(
            409,
            "version_conflict_engine_exception",
            "a notification config of that id already exists",
        ),
        MetaError::Contended => refuse(
            409,
            "version_conflict_engine_exception",
            "the notification configs are being changed, try again",
        ),
        // ALLOC: error path; one message.
        other => refuse(500, "notifications_store_failure", other.to_string()),
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The summary a catalogue keeps of a stored config: nothing secret.
fn summary(stored: &Value) -> Value {
    let c = &stored["config"];
    json!({
        "config_id": stored["config_id"],
        "name": c["name"],
        "description": c["description"],
        "config_type": c["config_type"],
        "is_enabled": c["is_enabled"],
        "created_time_ms": stored["created_time_ms"],
        "last_updated_time_ms": stored["last_updated_time_ms"],
        "seq": stored["seq"],
    })
}

/// The plugin's body for one config.
fn shown(stored: &Value) -> Value {
    json!({
        "config_id": stored["config_id"],
        "last_updated_time_ms": stored["last_updated_time_ms"],
        "created_time_ms": stored["created_time_ms"],
        "config": stored["config"],
    })
}

/// An id for a config no client named: `new_id` without a leading `_`, which
/// a set refuses.
fn fresh_id() -> Result<String, RestError> {
    let mut id = oss_meta::new_id().map_err(|_| {
        refuse(
            500,
            "notifications_store_failure",
            "no randomness for an id",
        )
    })?;
    if id.starts_with('_') {
        id.replace_range(..1, "n");
    }
    Ok(id)
}

impl NotificationsService {
    /// A service over the configs under `base` in `store`; a request holds
    /// up to `buffer_bytes` for a marker.
    pub fn new(store: &Arc<dyn BlobStore>, base: &str, buffer_bytes: usize) -> Self {
        Self {
            store: Arc::clone(store),
            // ALLOC: one copy of the base, once.
            base: base.to_owned(),
            buffer_bytes,
            allowed: config::TYPES.to_vec(),
            deliverer: None,
        }
    }

    /// Serves only these config types: a config of another is refused at put.
    #[must_use]
    pub fn with_allowed_types(mut self, allowed: Vec<&'static str>) -> Self {
        self.allowed = allowed;
        self
    }

    /// Sends the test route's message through `deliverer`; without one the
    /// route answers 501.
    #[must_use]
    pub fn with_deliverer(mut self, deliverer: std::sync::Arc<Deliverer>) -> Self {
        self.deliverer = Some(deliverer);
        self
    }

    fn set(&self, kind: DocKind) -> Result<DocSet, RestError> {
        DocSet::new(Arc::clone(&self.store), &self.base, kind).map_err(|e| failed(&e))
    }

    fn marker(&self, req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
        Ok(vec![0u8; self.buffer_bytes])
    }

    fn doc_buffer(
        req: &mut Request<'_>,
        kind: DocKind,
        copies: usize,
    ) -> Result<Vec<u8>, RestError> {
        let bytes = kind.max_doc_bytes();
        req.body().reserve(bytes.saturating_mul(copies))?;
        // ALLOC: one document buffer a request, the kind's bound, reserved above.
        Ok(vec![0u8; bytes])
    }

    fn id_of(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("id")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("a config id is required"))
    }

    fn stored(
        &self,
        id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Option<Value>, RestError> {
        match self.set(DocKind::NotificationConfigs)?.get(id, marker, doc) {
            Ok(Some(n)) => serde_json::from_slice(doc.get(..n).unwrap_or_default())
                .map(Some)
                .map_err(|_| {
                    refuse(
                        500,
                        "notifications_store_failure",
                        "a stored config is unreadable",
                    )
                }),
            Ok(None) => Ok(None),
            Err(e) => Err(failed(&e)),
        }
    }

    /// Applies `change` to the catalogue's entries by a rebuild on every swap
    /// attempt; `change` answers whether it changed anything.
    fn catalogue_update(
        &self,
        req: &mut Request<'_>,
        change: &mut dyn FnMut(&mut Vec<Value>) -> bool,
    ) -> Result<(), RestError> {
        let kind = DocKind::NotificationCatalogue;
        let mut marker = self.marker(req)?;
        // The document read and the one rebuilt are both held.
        let mut doc = Self::doc_buffer(req, kind, 2)?;
        self.set(kind)?
            .update(CATALOGUE, &mut marker, &mut doc, &mut |old| {
                let mut entries = entries_of(old)?;
                if !change(&mut entries) {
                    return Ok(None);
                }
                entries.sort_by(|a, b| a["config_id"].as_str().cmp(&b["config_id"].as_str()));
                // ALLOC: the catalogue's text, at most the kind's bound, checked by the put.
                Ok(Some(json!({"entries": entries}).to_string().into_bytes()))
            })
            .map_err(|e| failed(&e))
    }

    /// `POST /configs`: a config with a generated id, or the one it names.
    pub(crate) fn create(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let raw = read_body(
            req,
            MAX_CONFIG_BYTES,
            "the body is over the limit for a config",
        )?;
        let body: Value =
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?;
        let named = match body.get("config_id") {
            None => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(bad("[config_id] must be a string")),
        };
        let id = match named {
            Some(id) => id,
            None => fresh_id()?,
        };
        let stored = self.build(&id, &body, None)?;
        // ALLOC: the document's text, at most the config bound.
        let bytes = stored.to_string().into_bytes();
        let mut marker = self.marker(req)?;
        self.set(DocKind::NotificationConfigs)?
            .put(&id, &bytes, true, &mut marker)
            .map_err(|e| failed(&e))?;
        let entry = summary(&stored);
        self.catalogue_update(req, &mut |entries| upsert(entries, &entry))?;
        resp.send(200, JSON, json!({"config_id": id}).to_string().as_bytes())
    }

    /// `PUT /configs/{id}`: a replace; the config must exist.
    pub(crate) fn replace(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let raw = read_body(
            req,
            MAX_CONFIG_BYTES,
            "the body is over the limit for a config",
        )?;
        let body: Value =
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?;
        if body
            .get("config_id")
            .is_some_and(|v| v.as_str() != Some(&id))
        {
            return Err(bad("[config_id] must match the id in the path"));
        }
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req, DocKind::NotificationConfigs, 1)?;
        let old = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| not_found(&id))?;
        let stored = self.build(&id, &body, Some(&old))?;
        // ALLOC: the document's text, at most the config bound.
        let bytes = stored.to_string().into_bytes();
        self.set(DocKind::NotificationConfigs)?
            .put(&id, &bytes, false, &mut marker)
            .map_err(|e| failed(&e))?;
        let entry = summary(&stored);
        self.catalogue_update(req, &mut |entries| upsert(entries, &entry))?;
        resp.send(200, JSON, json!({"config_id": id}).to_string().as_bytes())
    }

    /// The stored document for `body`'s config: checked, stamped, and for a
    /// replace carrying the creation time and the next count of `old`.
    fn build(&self, id: &str, body: &Value, old: Option<&Value>) -> Result<Value, RestError> {
        let config = body
            .get("config")
            .ok_or_else(|| bad("the body needs a [config] object"))?;
        let checked = config::checked(config, &self.allowed).map_err(bad)?;
        let now = now_millis();
        let (created, seq) = old.map_or((now, 0), |o| {
            (
                o["created_time_ms"].as_u64().unwrap_or(now),
                o["seq"].as_i64().unwrap_or(0).saturating_add(1),
            )
        });
        Ok(json!({
            "config_id": id,
            "seq": seq,
            "created_time_ms": created,
            "last_updated_time_ms": now,
            "config": Value::Object(checked),
        }))
    }

    /// Deletes `id`: its summary first, then the config. Whether there was one.
    fn remove(&self, req: &mut Request<'_>, id: &str) -> Result<bool, RestError> {
        self.catalogue_update(req, &mut |entries| {
            let before = entries.len();
            entries.retain(|e| e["config_id"].as_str() != Some(id));
            entries.len() != before
        })?;
        let mut marker = self.marker(req)?;
        self.set(DocKind::NotificationConfigs)?
            .delete(id, &mut marker)
            .map_err(|e| failed(&e))
    }

    /// `DELETE /configs/{id}`.
    pub(crate) fn delete_one(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        if !self.remove(req, &id)? {
            return Err(not_found(&id));
        }
        resp.send(200, JSON, deleted(&[(id, "OK")]).to_string().as_bytes())
    }

    /// `DELETE /configs?config_id=..&config_id_list=a,b`.
    pub(crate) fn delete_many(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let ids = list::ids_of(req)?;
        if ids.is_empty() {
            return Err(bad("[config_id] or [config_id_list] is required"));
        }
        let mut done = Vec::with_capacity(ids.len());
        for id in ids {
            let status = if self.remove(req, &id)? {
                "OK"
            } else {
                "NOT_FOUND"
            };
            done.push((id, status));
        }
        resp.send(200, JSON, deleted(&done).to_string().as_bytes())
    }

    /// `GET /features`.
    pub(crate) fn features(
        &self,
        _req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let body = json!({
            "allowed_config_type_list": self.allowed,
            "plugin_features": {"tooltip_support": "true"},
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}

fn deleted(done: &[(String, &str)]) -> Value {
    let list: Map<String, Value> = done
        .iter()
        .map(|(id, s)| (id.clone(), Value::String((*s).to_owned())))
        .collect();
    json!({"delete_response_list": list})
}

/// The entries of a catalogue document, none for no document.
fn entries_of(doc: Option<&[u8]>) -> Result<Vec<Value>, MetaError> {
    let Some(doc) = doc else {
        return Ok(Vec::new());
    };
    let v: Value = serde_json::from_slice(doc)
        .map_err(|_| MetaError::InvalidArgument("the notification catalogue is unreadable"))?;
    match v.get("entries") {
        Some(Value::Array(a)) => Ok(a.clone()),
        _ => Err(MetaError::InvalidArgument(
            "the notification catalogue is unreadable",
        )),
    }
}

/// Puts `entry` in place of the entry of its id, or adds it.
fn upsert(entries: &mut Vec<Value>, entry: &Value) -> bool {
    let id = entry["config_id"].as_str();
    match entries.iter_mut().find(|e| e["config_id"].as_str() == id) {
        Some(existing) if existing == entry => return false,
        Some(existing) => *existing = entry.clone(),
        None => entries.push(entry.clone()),
    }
    true
}
