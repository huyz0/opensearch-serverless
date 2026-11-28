//! The routes that attach a policy to an index and say what is attached:
//! `add`, `remove` and `explain`. An index is managed by one document of the
//! `ism/managed/` set, named for the index.

use std::borrow::Cow;
use std::sync::Arc;

use oss_format::DocKind;
use oss_meta::{DescriptorStore, MetaError, ScheduleRegistry};
use oss_rest::{Exception, JSON, Meta, Request, Response, RestError};
use serde_json::{Value, json};

use super::{IsmService, bad, failed, now_millis, read_body, refuse};

/// The most bytes of an `add` body.
const MAX_BODY_BYTES: usize = 64 << 10;

const PLUGIN_KEY: &str = "index.plugins.index_state_management.policy_id";
const LEGACY_KEY: &str = "index.opendistro.index_state_management.policy_id";

/// The most indices one request names.
const MAX_NAMES: usize = 100;

/// The indices a request names, comma separated and literal.
fn literals(req: &Request<'_>) -> Result<Vec<String>, RestError> {
    let raw = req
        .path_param("index")
        .map(Cow::into_owned)
        .ok_or_else(|| bad("an index name is required"))?;
    let names: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect();
    if names.iter().any(|n| n.contains(['*', '?']) || n == "_all") {
        return Err(bad("name the indices literally: a wildcard is not served"));
    }
    if names.is_empty() || names.len() > MAX_NAMES {
        return Err(bad(format!("name 1 to {MAX_NAMES} indices")));
    }
    Ok(names)
}

/// What every attach and detach answers.
fn outcome(updated: u32, failed: &[(String, String, String)]) -> Value {
    let failed_indices: Vec<Value> = failed
        .iter()
        .map(|(name, uuid, reason)| json!({"index_name": name, "index_uuid": uuid, "reason": reason}))
        .collect();
    json!({
        "updated_indices": updated,
        "failures": !failed_indices.is_empty(),
        "failed_indices": failed_indices,
    })
}

fn missing_policy(id: &str) -> RestError {
    refuse(
        404,
        "status_exception",
        format!("Could not find policy={id}"),
    )
}

/// `IndexNotFoundException` for a name with no index record.
fn index_missing(index: &str) -> RestError {
    RestError::exception(
        404,
        Exception::new(
            "index_not_found_exception",
            format!("no such index [{index}]"),
        )
        .with_meta("index", Meta::Str(index.to_owned()))
        .with_meta("resource.id", Meta::Str(index.to_owned()))
        .with_meta("resource.type", Meta::Str("index_or_alias".to_owned()))
        .with_meta("index_uuid", Meta::Str("_na_".to_owned())),
    )
}

/// The `policy_id` of a body, or the plugin's 400.
fn policy_id_of(raw: &[u8]) -> Result<(String, Value), RestError> {
    let missing = || bad("Missing policy_id");
    if raw.is_empty() {
        return Err(missing());
    }
    let body: Value = serde_json::from_slice(raw).map_err(|e| bad(format!("invalid body: {e}")))?;
    let id = body
        .get("policy_id")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .ok_or_else(missing)?
        .to_owned();
    Ok((id, body))
}

impl IsmService {
    fn send(resp: &mut Response<'_>, body: &Value) -> Result<(), RestError> {
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    fn descriptors(&self) -> Result<&DescriptorStore, RestError> {
        self.descriptors.as_ref().ok_or_else(|| {
            refuse(
                501,
                "unsupported_operation",
                "this node has no index records",
            )
        })
    }

    /// The uuid and creation date of index `name`, `None` for no such index.
    fn index_record(
        &self,
        name: &str,
        marker: &mut [u8],
    ) -> Result<Option<(String, i64)>, RestError> {
        match self.descriptors()?.get(name, marker) {
            Ok(Some(r)) => Ok(Some((r.uuid.to_string(), r.created_at_millis))),
            Ok(None) => Ok(None),
            Err(e) => Err(failed(&e)),
        }
    }

    /// The indices `names` all have records, with their uuids; a missing one
    /// is the plugin's 404.
    fn existing(&self, names: &[String], marker: &mut [u8]) -> Result<Vec<String>, RestError> {
        // ALLOC: one uuid a named index, at most MAX_NAMES.
        let mut uuids = Vec::with_capacity(names.len());
        for name in names {
            let (uuid, _) = self
                .index_record(name, marker)?
                .ok_or_else(|| index_missing(name))?;
            uuids.push(uuid);
        }
        Ok(uuids)
    }

    /// `POST /add/{index}` with `{"policy_id", "state"?}`.
    pub(crate) fn add(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let names = literals(req)?;
        let raw = read_body(req, MAX_BODY_BYTES, "the body is over 64 KiB")?;
        let (policy_id, body) = policy_id_of(&raw)?;
        let state = body.get("state").and_then(Value::as_str);
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let stored = self
            .stored(&policy_id, &mut marker, &mut doc)?
            .ok_or_else(|| missing_policy(&policy_id))?;
        let (mut updated, mut refused) = (0u32, Vec::new());
        for index in &names {
            // The plugin skips an index it cannot find, counting nothing.
            let Some((uuid, created)) = self.index_record(index, &mut marker)? else {
                continue;
            };
            match self.attach(
                index,
                &uuid,
                created,
                &policy_id,
                &stored,
                state,
                &mut marker,
            )? {
                None => updated = updated.saturating_add(1),
                Some(reason) => refused.push((index.clone(), uuid, reason)),
            }
        }
        Self::send(resp, &outcome(updated, &refused))
    }

    /// Manages `index` by the policy `stored`, starting in `state` or the
    /// policy's default: `None` when it did, else why it could not.
    ///
    /// # Errors
    ///
    /// 429 when the tenant holds the most managed indices, and a failed
    /// store call.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn attach(
        &self,
        index: &str,
        uuid: &str,
        created: i64,
        policy_id: &str,
        stored: &Value,
        state: Option<&str>,
        marker: &mut [u8],
    ) -> Result<Option<String>, RestError> {
        let policy = &stored["policy"];
        let first = state.unwrap_or_else(|| policy["default_state"].as_str().unwrap_or_default());
        let known = policy["states"]
            .as_array()
            .is_some_and(|s| s.iter().any(|x| x["name"] == first));
        if !known {
            return Ok(Some(format!(
                "state [{first}] is not a state of policy [{policy_id}]"
            )));
        }
        let now = now_millis();
        let managed = json!({
            "policy_id": policy_id,
            "policy_seq_no": stored["seq_no"],
            "index": index,
            "index_uuid": uuid,
            "index_creation_date": created,
            "state": {"name": first, "start_time": now},
            "enabled": true,
            "enabled_time": now,
        });
        // ALLOC: the document's text, a few hundred bytes.
        let bytes = managed.to_string().into_bytes();
        match self
            .set_of(DocKind::IsmManaged)?
            .put_counted(index, &bytes, true, marker)
        {
            Ok(_) => {
                // The runner finds tenants in the registry, so an add that
                // wrote the record names the tenant (one shard swap).
                ScheduleRegistry::in_namespace(
                    Arc::clone(&self.store),
                    &self.base,
                    crate::ism_run::schedule::NAMESPACE,
                )
                .and_then(|r| r.register(crate::ism_run::schedule::TENANT))
                .map_err(|e| failed(&e))?;
                Ok(None)
            }
            Err(MetaError::AlreadyExists) => Ok(Some(
                "This index already has a policy, use the update policy API to update index policies"
                    .to_owned(),
            )),
            Err(MetaError::TooManyDocs) => Err(refuse(
                429,
                "too_many_managed_indices",
                "a tenant holds at most 1000 managed indices",
            )),
            Err(e) => Err(failed(&e)),
        }
    }

    /// Writes `record` over the managed index's document.
    fn replace_managed(
        &self,
        index: &str,
        record: &Value,
        marker: &mut [u8],
    ) -> Result<(), RestError> {
        // ALLOC: the document's text, a few hundred bytes.
        let bytes = record.to_string().into_bytes();
        self.set_of(DocKind::IsmManaged)?
            .put_counted(index, &bytes, false, marker)
            .map(|_| ())
            .map_err(|e| failed(&e))
    }

    /// `POST /change_policy/{index}` with `{"policy_id", "state"?}`: the
    /// change is recorded on the managed index for the runner to apply at its
    /// next step, as the plugin applies it on the next job run.
    pub(crate) fn change_policy(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let names = literals(req)?;
        let raw = read_body(req, MAX_BODY_BYTES, "the body is over 64 KiB")?;
        let (policy_id, body) = policy_id_of(&raw)?;
        let state = body.get("state").and_then(Value::as_str);
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let stored = self
            .stored(&policy_id, &mut marker, &mut doc)?
            .ok_or_else(|| missing_policy(&policy_id))?;
        let uuids = self.existing(&names, &mut marker)?;
        if let Some(state) = state {
            let known = stored["policy"]["states"]
                .as_array()
                .is_some_and(|s| s.iter().any(|x| x["name"] == state));
            if !known {
                let reason = format!("state [{state}] is not a state of policy [{policy_id}]");
                let all: Vec<_> = names
                    .iter()
                    .zip(&uuids)
                    .map(|(n, u)| (n.clone(), u.clone(), reason.clone()))
                    .collect();
                return Self::send(resp, &outcome(0, &all));
            }
        }
        let (mut updated, mut refused) = (0u32, Vec::new());
        for (index, uuid) in names.iter().zip(&uuids) {
            let Some(mut record) =
                self.stored_in(DocKind::IsmManaged, index, &mut marker, &mut doc)?
            else {
                let reason = "This index does not have a policy to update";
                refused.push((index.clone(), uuid.clone(), reason.to_owned()));
                continue;
            };
            record["change_policy"] = json!({"policy_id": policy_id, "state": state});
            self.replace_managed(index, &record, &mut marker)?;
            updated = updated.saturating_add(1);
        }
        Self::send(resp, &outcome(updated, &refused))
    }

    /// `POST /retry/{index}` with an optional `{"state"}`: asks the runner to
    /// run the failed step again.
    pub(crate) fn retry(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let names = literals(req)?;
        let raw = read_body(req, MAX_BODY_BYTES, "the body is over 64 KiB")?;
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?
        };
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let uuids = self.existing(&names, &mut marker)?;
        let (mut updated, mut refused) = (0u32, Vec::new());
        for (index, uuid) in names.iter().zip(&uuids) {
            let mut no =
                |reason: &str| refused.push((index.clone(), uuid.clone(), reason.to_owned()));
            let Some(mut record) =
                self.stored_in(DocKind::IsmManaged, index, &mut marker, &mut doc)?
            else {
                no("This index does not have a policy");
                continue;
            };
            // The runner writes `failed` with its first step's outcome; before
            // that the index has no metadata, as the plugin says.
            if record.get("failed").is_none() {
                no("This index has no metadata information");
                continue;
            }
            if record["failed"] != true {
                no("This index is not in a failed state");
                continue;
            }
            record["retry_requested"] = json!(true);
            if let Some(state) = body.get("state").and_then(Value::as_str) {
                record["retry_state"] = json!(state);
            }
            self.replace_managed(index, &record, &mut marker)?;
            updated = updated.saturating_add(1);
        }
        Self::send(resp, &outcome(updated, &refused))
    }

    /// `POST /remove/{index}`.
    pub(crate) fn remove(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let names = literals(req)?;
        let mut marker = self.marker(req)?;
        let uuids = self.existing(&names, &mut marker)?;
        let (mut updated, mut refused) = (0u32, Vec::new());
        for (index, uuid) in names.iter().zip(&uuids) {
            let gone = self
                .set_of(DocKind::IsmManaged)?
                .delete(index, &mut marker)
                .map_err(|e| failed(&e))?;
            if gone {
                updated = updated.saturating_add(1);
            } else {
                let reason = "This index does not have a policy to remove";
                refused.push((index.clone(), uuid.clone(), reason.to_owned()));
            }
        }
        Self::send(resp, &outcome(updated, &refused))
    }

    /// `GET|POST /explain/{index}`: `show_policy` adds the policy, and
    /// `validate_action` the plugin's validation answer.
    pub(crate) fn explain(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let names = literals(req)?;
        let show_policy = req.param("show_policy").is_some_and(|v| v == "true");
        let validate = req.param("validate_action").is_some_and(|v| v == "true");
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let mut body = serde_json::Map::new();
        let mut total = 0u32;
        for index in names {
            let managed = self.stored_in(DocKind::IsmManaged, &index, &mut marker, &mut doc)?;
            let entry = match managed {
                Some(m) => {
                    total = u32::saturating_add(total, 1);
                    let mut entry = json!({
                        PLUGIN_KEY: m["policy_id"],
                        LEGACY_KEY: m["policy_id"],
                        "index": m["index"],
                        "index_uuid": m["index_uuid"],
                        "policy_id": m["policy_id"],
                        "enabled": m["enabled"],
                    });
                    for key in ["change_policy", "retry_requested"] {
                        if !m[key].is_null() {
                            entry[key] = m[key].clone();
                        }
                    }
                    if validate {
                        entry["validate"] = json!({
                            "validation_message": "Starting Validation",
                            "validation_status": "passed",
                        });
                    }
                    if show_policy {
                        let id = m["policy_id"].as_str().unwrap_or_default();
                        if let Some(p) = self.stored(id, &mut marker, &mut doc)? {
                            entry["policy"] = p["policy"].clone();
                        }
                    }
                    entry
                }
                None => {
                    if self.index_record(&index, &mut marker)?.is_none() {
                        return Err(refuse(
                            400,
                            "action_request_validation_exception",
                            "Validation Failed: 1: no documents to get;",
                        ));
                    }
                    json!({PLUGIN_KEY: null, LEGACY_KEY: null, "enabled": null})
                }
            };
            body.insert(index, entry);
        }
        body.insert("total_managed_indices".to_owned(), json!(total));
        Self::send(resp, &Value::Object(body))
    }
}
