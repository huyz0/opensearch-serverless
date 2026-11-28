//! `models/{id}/_predict`: a remote model's call through its connector.
//!
//! The model and its connector are read once and kept, opened, for the cache's TTL, so a
//! warm predict makes no object-store request; another node's change to either is seen
//! when the entry expires. Nothing is held between calls but the opened connector.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use oss_format::DocKind;
use oss_ml::predict::{Failure, Remote, amz_date};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::Value;

use super::{MlService, bad, now_millis, refuse};

/// How long an opened connector is kept.
pub(super) const CACHE_TTL: Duration = Duration::from_secs(30);
/// The most entries kept.
const CACHE_ENTRIES: usize = 128;
/// The most bytes of a remote error shown.
const ERROR_BYTES: usize = 512;

/// The opened connectors of the models this node predicted with, by model id.
#[derive(Default)]
pub(super) struct Cache(Mutex<HashMap<String, (Instant, Arc<Remote>)>>);

impl Cache {
    fn get(&self, id: &str, ttl: Duration) -> Option<Arc<Remote>> {
        let map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        map.get(id)
            .filter(|(at, _)| at.elapsed() < ttl)
            .map(|(_, r)| Arc::clone(r))
    }

    fn put(&self, id: &str, remote: Arc<Remote>, ttl: Duration) {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if map.len() >= CACHE_ENTRIES {
            map.retain(|_, (at, _)| at.elapsed() < ttl);
        }
        if map.len() >= CACHE_ENTRIES {
            map.clear();
        }
        // ALLOC: one entry a model, at most CACHE_ENTRIES.
        map.insert(id.to_owned(), (Instant::now(), remote));
    }

    /// Forgets one model's entry.
    pub(super) fn forget(&self, id: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }

    /// Forgets every entry: a connector changed.
    pub(super) fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

fn answered(f: Failure) -> RestError {
    let kind = match f.status {
        400 => "illegal_argument_exception",
        408 => "timeout_exception",
        429 => "es_rejected_execution_exception",
        501 => "not_implemented",
        s if s >= 500 => "status_exception",
        _ => "status_exception",
    };
    refuse(f.status, kind, f.reason)
}

/// A parameter as a connector template takes it: text as it is, anything else as JSON.
fn param(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

impl MlService {
    /// The opened connector of model `id`: from the cache, or read (the model 2 GETs, its
    /// connector 2, the master key once a node).
    fn remote_of(
        &self,
        id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Arc<Remote>, RestError> {
        if let Some(hit) = self.cache.get(id, self.cache_ttl) {
            return Ok(hit);
        }
        let model = self
            .stored_in(DocKind::MlModels, id, marker, doc)?
            .ok_or_else(|| super::models::missing(id))?;
        let cid = model
            .get("connector_id")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("the model has no connector"))?;
        let connector = self
            .stored_in(DocKind::MlConnectors, cid, marker, doc)?
            .ok_or_else(|| super::models::connector_missing(cid))?;
        let remote = Remote::open(&connector, self.master_key()?).map_err(answered)?;
        let remote = Arc::new(remote);
        self.cache.put(id, Arc::clone(&remote), self.cache_ttl);
        Ok(remote)
    }

    fn predict(
        &self,
        id: &str,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let body = Self::body_or_empty(req)?;
        let parameters: Vec<(String, Option<String>)> = match body.get("parameters") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Object(p)) => p.iter().map(|(k, v)| (k.clone(), param(v))).collect(),
            Some(_) => return Err(bad("[parameters] must be an object")),
        };
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let remote = self.remote_of(id, &mut marker, &mut doc)?;
        drop((marker, doc));
        let secs = now_millis() / 1_000;
        let call = remote
            .prepare(&parameters, &amz_date(secs))
            .map_err(answered)?;
        // A stored URL is checked again: the trusted endpoints may have changed since.
        if !self.trusted.allows(&call.url) {
            return Err(bad(
                "Connector URL is not matching the trusted connector endpoint regex",
            ));
        }
        let answer = self
            .outbound
            .send(&call.method, &call.url, &call.headers, call.body.clone())
            .map_err(answered)?;
        if !(200..300).contains(&answer.status) {
            let mut end = answer.body.len().min(ERROR_BYTES);
            while !answer.body.is_char_boundary(end) {
                end = end.saturating_sub(1);
            }
            return Err(refuse(
                answer.status,
                "status_exception",
                // ALLOC: error path; at most ERROR_BYTES of the remote's answer.
                format!("Error from remote service: {}", &answer.body[..end]),
            ));
        }
        let out = remote.read(&call, &answer.body).map_err(answered)?;
        resp.send(200, JSON, out.as_bytes())
    }

    /// `POST /models/{id}/_predict`.
    pub(crate) fn predict_model(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::model_id(req)?;
        self.predict(&id, req, resp)
    }

    /// `POST /_predict/{algorithm}/{model_id}`: the same call, naming the algorithm.
    pub(crate) fn predict_algorithm(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::model_id(req)?;
        let algorithm = req
            .path_param("algorithm")
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default();
        if !algorithm.eq_ignore_ascii_case("remote") {
            return Err(refuse(
                501,
                "not_implemented",
                // ALLOC: error path; one message.
                format!("predict with algorithm [{algorithm}] is not served by this node yet"),
            ));
        }
        self.predict(&id, req, resp)
    }
}
