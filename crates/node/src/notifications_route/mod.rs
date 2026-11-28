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
mod notifier;
mod routes;
mod test_send;
mod write;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::BlobStore;
use oss_format::DocKind;
use oss_meta::{DocSet, MetaError};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Map, Value, json};

use crate::route_util::refuse;
pub use deliver::{Deliverer, Delivery, Limits};
pub use egress::{EgressPolicy, Lookup, permitted, system_lookup};
pub use notifier::{Notifier, NotifierCounts};
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
        format!("NotificationConfig {id} not found"),
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
            "status_exception",
            "a notification config of that id already exists",
        ),
        MetaError::Contended => refuse(
            409,
            "status_exception",
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
    let next = entries
        .iter()
        .filter_map(|e| e["ord"].as_u64())
        .max()
        .map_or(0, |m| m.saturating_add(1));
    match entries.iter_mut().find(|e| e["config_id"].as_str() == id) {
        Some(existing) => {
            let mut entry = entry.clone();
            entry["ord"] = existing["ord"].clone();
            if *existing == entry {
                return false;
            }
            *existing = entry;
        }
        None => {
            let mut entry = entry.clone();
            entry["ord"] = Value::from(next);
            entries.push(entry);
        }
    }
    true
}
