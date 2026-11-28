//! The routes that change configs: create, replace and delete.

use oss_format::DocKind;
use oss_meta::MetaError;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::{
    MAX_CONFIG_BYTES, NotificationsService, bad, config, deleted, failed, fresh_id, list,
    not_found, now_millis, summary, upsert,
};
use crate::route_util::{read_body, refuse};

impl NotificationsService {
    /// `POST /configs`: a config with a generated id, or the one it names.
    pub(crate) fn create(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let body = Self::json_body(req)?;
        let named = match body.get("config_id") {
            None => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(bad("[config_id] must be a string")),
        };
        let id = match named {
            Some(id) => id,
            None => fresh_id()?,
        };
        let stored = self.build(req, &id, &body, None)?;
        // ALLOC: the document's text, at most the config bound.
        let bytes = stored.to_string().into_bytes();
        let mut marker = self.marker(req)?;
        self.set(DocKind::NotificationConfigs)?
            .put(&id, &bytes, true, &mut marker)
            .map_err(|e| match e {
                MetaError::AlreadyExists => refuse(
                    409,
                    "status_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "Document version conflict putting {id} in index .opensearch-notifications-config"
                    ),
                ),
                other => failed(&other),
            })?;
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
        let body = Self::json_body(req)?;
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
        let stored = self.build(req, &id, &body, Some(&old))?;
        // ALLOC: the document's text, at most the config bound.
        let bytes = stored.to_string().into_bytes();
        self.set(DocKind::NotificationConfigs)?
            .put(&id, &bytes, false, &mut marker)
            .map_err(|e| failed(&e))?;
        let entry = summary(&stored);
        self.catalogue_update(req, &mut |entries| upsert(entries, &entry))?;
        resp.send(200, JSON, json!({"config_id": id}).to_string().as_bytes())
    }

    /// The request's body as JSON; an empty body is the plugin's parse error.
    fn json_body(req: &mut Request<'_>) -> Result<Value, RestError> {
        let raw = read_body(
            req,
            MAX_CONFIG_BYTES,
            "the body is over the limit for a config",
        )?;
        if raw.iter().all(u8::is_ascii_whitespace) {
            return Err(refuse(400, "parse_exception", "request body is required"));
        }
        serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))
    }

    /// Fails with the plugin's 404 when a named account or group is no config
    /// of its kind.
    fn require(&self, req: &mut Request<'_>, refs: &config::Refs) -> Result<(), RestError> {
        if refs.accounts.is_empty() && refs.groups.is_empty() {
            return Ok(());
        }
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req, DocKind::NotificationConfigs, 1)?;
        let mut missing: Vec<&str> = Vec::new();
        let wanted = refs
            .accounts
            .iter()
            .map(|id| (id, &["smtp_account", "ses_account"][..]))
            .chain(refs.groups.iter().map(|id| (id, &["email_group"][..])));
        for (id, kinds) in wanted {
            let found = self.stored(id, &mut marker, &mut doc)?;
            let ok = found
                .as_ref()
                .and_then(|s| s["config"]["config_type"].as_str())
                .is_some_and(|t| kinds.contains(&t));
            if !ok {
                missing.push(id);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        // ALLOC: error path; one message.
        Err(refuse(
            404,
            "status_exception",
            format!("Config IDs not found:[{}]", missing.join(", ")),
        ))
    }

    /// The stored document for `body`'s config: checked, stamped, and for a
    /// replace carrying the creation time and the next count of `old`.
    fn build(
        &self,
        req: &mut Request<'_>,
        id: &str,
        body: &Value,
        old: Option<&Value>,
    ) -> Result<Value, RestError> {
        let config = body
            .get("config")
            .ok_or_else(|| bad("the body needs a [config] object"))?;
        let (checked, refs) = config::checked(config, &self.allowed)?;
        if let Some(o) = old
            && o["config"]["config_type"] != checked["config_type"]
        {
            return Err(refuse(
                409,
                "status_exception",
                "Config type cannot be changed after creation",
            ));
        }
        self.require(req, &refs)?;
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

    /// `DELETE /configs?config_id=..&config_id_list=a,b`: every named config
    /// or none, as the plugin does.
    pub(crate) fn delete_many(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let ids = list::ids_of(req)?;
        if ids.is_empty() {
            return resp.send(
                400,
                "text/plain; charset=UTF-8",
                b"either config_id or config_id_list is required",
            );
        }
        let mut marker = self.marker(req)?;
        let held = self
            .set(DocKind::NotificationConfigs)?
            .names(&mut marker)
            .map_err(|e| failed(&e))?;
        let missing: Vec<&str> = ids
            .iter()
            .map(String::as_str)
            .filter(|id| !held.iter().any(|h| h == id))
            .collect();
        if !missing.is_empty() {
            // ALLOC: error path; one message.
            return Err(not_found(&format!("[{}]", missing.join(", "))));
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
}
