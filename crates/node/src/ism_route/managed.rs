//! The routes that attach a policy to an index and say what is attached:
//! `add`, `remove` and `explain`. An index is managed by one document of the
//! `ism/managed/` set, named for the index.

use std::borrow::Cow;

use oss_format::DocKind;
use oss_meta::MetaError;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::{IsmService, bad, failed, now_millis, read_body, refuse};

/// The most bytes of an `add` body.
const MAX_BODY_BYTES: usize = 64 << 10;

const PLUGIN_KEY: &str = "index.plugins.index_state_management.policy_id";
const LEGACY_KEY: &str = "index.opendistro.index_state_management.policy_id";

/// An index name that stands for exactly one index.
fn literal(req: &Request<'_>) -> Result<String, RestError> {
    let name = req
        .path_param("index")
        .map(Cow::into_owned)
        .ok_or_else(|| bad("an index name is required"))?;
    if name.contains(['*', ',', '?']) || name == "_all" {
        return Err(bad(
            "name the index literally: a wildcard or a list of indices is not served",
        ));
    }
    Ok(name)
}

/// What every attach and detach answers.
fn outcome(updated: u32, failed_index: Option<(&str, &str, &str)>) -> Value {
    let failed_indices: Vec<Value> = failed_index
        .map(|(name, uuid, reason)| json!({"index_name": name, "index_uuid": uuid, "reason": reason}))
        .into_iter()
        .collect();
    json!({
        "updated_indices": updated,
        "failures": !failed_indices.is_empty(),
        "failed_indices": failed_indices,
    })
}

impl IsmService {
    fn send(resp: &mut Response<'_>, body: &Value) -> Result<(), RestError> {
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// `POST /add/{index}` with `{"policy_id", "state"?}`.
    pub(crate) fn add(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let index = literal(req)?;
        let raw = read_body(req, MAX_BODY_BYTES, "the body is over 64 KiB")?;
        let body: Value =
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?;
        let policy_id = body
            .get("policy_id")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| bad("[policy_id] is required"))?;
        let state = body.get("state").and_then(Value::as_str);
        let descriptors = self.descriptors.as_ref().ok_or_else(|| {
            refuse(
                501,
                "unsupported_operation",
                "this node has no index records",
            )
        })?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let Some(stored) = self.stored(policy_id, &mut marker, &mut doc)? else {
            return Self::send(resp, &outcome(0, Some((&index, "", "Policy not found"))));
        };
        let (uuid, created) = match descriptors.get(&index, &mut marker) {
            Ok(Some(record)) => (record.uuid.to_string(), record.created_at_millis),
            Ok(None) => {
                let reason = format!("no such index [{index}]");
                return Self::send(resp, &outcome(0, Some((&index, "", &reason))));
            }
            Err(e) => return Err(failed(&e)),
        };
        let policy = &stored["policy"];
        let first = state.unwrap_or_else(|| policy["default_state"].as_str().unwrap_or_default());
        let known = policy["states"]
            .as_array()
            .is_some_and(|s| s.iter().any(|x| x["name"] == first));
        if !known {
            let reason = format!("state [{first}] is not a state of policy [{policy_id}]");
            return Self::send(resp, &outcome(0, Some((&index, &uuid, &reason))));
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
            .put_counted(&index, &bytes, true, &mut marker)
        {
            Ok(_) => Self::send(resp, &outcome(1, None)),
            Err(MetaError::AlreadyExists) => Self::send(
                resp,
                &outcome(
                    0,
                    Some((
                        &index,
                        &uuid,
                        "This index already has a policy, use the update policy API to update index policies",
                    )),
                ),
            ),
            Err(MetaError::TooManyDocs) => Err(refuse(
                429,
                "too_many_managed_indices",
                "a tenant holds at most 1000 managed indices",
            )),
            Err(e) => Err(failed(&e)),
        }
    }

    /// `POST /remove/{index}`.
    pub(crate) fn remove(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let index = literal(req)?;
        let mut marker = self.marker(req)?;
        let gone = self
            .set_of(DocKind::IsmManaged)?
            .delete(&index, &mut marker)
            .map_err(|e| failed(&e))?;
        let body = if gone {
            outcome(1, None)
        } else {
            outcome(
                0,
                Some((&index, "", "This index does not have a policy to remove")),
            )
        };
        Self::send(resp, &body)
    }

    /// `GET|POST /explain/{index}`.
    pub(crate) fn explain(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let index = literal(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let managed = self.stored_in(DocKind::IsmManaged, &index, &mut marker, &mut doc)?;
        let (entry, total) = match managed {
            Some(m) => (
                json!({
                    PLUGIN_KEY: m["policy_id"],
                    LEGACY_KEY: m["policy_id"],
                    "index": m["index"],
                    "index_uuid": m["index_uuid"],
                    "policy_id": m["policy_id"],
                    "policy_seq_no": m["policy_seq_no"],
                    "policy_primary_term": 1,
                    "rolled_over": false,
                    "index_creation_date": m["index_creation_date"],
                    "state": m["state"],
                    "enabled": m["enabled"],
                }),
                1,
            ),
            None => (
                json!({PLUGIN_KEY: null, LEGACY_KEY: null, "enabled": null}),
                0,
            ),
        };
        let mut body = serde_json::Map::new();
        body.insert(index, entry);
        body.insert("total_managed_indices".to_owned(), json!(total));
        Self::send(resp, &Value::Object(body))
    }
}
