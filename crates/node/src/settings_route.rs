//! `PUT /{index}/_settings`: change the settings of one index.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/indexadmin/SettingsUpdateHandler.java`.
//! The decisions (body shapes, unknown keys, refusals, merge) are
//! `oss-coordinator`'s `indices::settings_update`; this route classifies each
//! key against the node's registry, swaps the descriptor with the merged
//! settings and bumps `settings_version`.

use std::sync::Arc;

use oss_coordinator::indices::settings_update::{ATTEMPTS, Kind, Scope, Setting, merge, parse};
use oss_doc::mapping::json::{Json, parse as parse_json};
use oss_meta::{DescriptorStore, Target};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::route_util::{read_body, refuse};

/// The most bytes of a settings body.
const MAX_BODY_BYTES: usize = 1 << 20;

/// The query parameters an update accepts.
const PARAMS: &[&str] = &[
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
    "preserve_existing",
];

const ACKNOWLEDGED: &str = "{\"acknowledged\":true}";

/// Core settings that change what a node does, and are refused until each is
/// implemented: storing one and ignoring it would break the promise the
/// client made by setting it.
fn behaviour_changing(key: &str) -> bool {
    matches!(
        key,
        "index.blocks.read" | "index.blocks.metadata" | "index.max_result_window"
    )
}

/// The write blocks a bulk reads from the descriptor.
fn write_block_key(key: &str) -> bool {
    matches!(
        key,
        "index.blocks.write" | "index.blocks.read_only" | "index.blocks.read_only_allow_delete"
    )
}

/// What this node's registry says of an index setting.
struct Registry {
    pipelines: bool,
}

impl Scope for Registry {
    fn classify(&self, key: &str, value: Option<&str>) -> Kind {
        // Core declares these; the update's own refusals answer for them.
        if behaviour_changing(key)
            || matches!(
                key,
                "index.number_of_shards"
                    | "index.number_of_replicas"
                    | "index.search.default_pipeline"
                    | "index.serverless.shard_mode"
            )
        {
            return Kind::Core { dynamic: true };
        }
        if write_block_key(key) {
            return Kind::Ours(match value {
                None | Some("true" | "false") => Ok(()),
                Some(other) => Err(format!(
                    "failed to parse value [{other}] for setting [{key}] as a boolean: only [true] or [false] are allowed"
                )),
            });
        }
        if crate::create_settings::is_static(key) || crate::index_route::is_bucket_key(key) {
            return Kind::Core { dynamic: false };
        }
        let Some(value) = value else {
            return if crate::create_settings::is_known(key) {
                Kind::Ours(Ok(()))
            } else {
                Kind::Unknown
            };
        };
        if !crate::create_settings::is_known(key) {
            return Kind::Unknown;
        }
        let pair = [(key.to_owned(), Some(value.to_owned()))];
        // The shard count a pairwise check needs is not read for a dynamic key.
        Kind::Ours(
            crate::create_settings::checked(&pair, 1, self.pipelines)
                .map(|_| ())
                .map_err(|e| {
                    e.as_exception()
                        .map_or_else(String::new, |x| x.reason().to_owned())
                }),
        )
    }
}

/// A setting's value as the text a request gives.
fn text_of(value: &Json) -> String {
    match value {
        Json::Str(s) | Json::Num(s) => s.clone(),
        Json::Bool(b) => b.to_string(),
        other => other.to_json(),
    }
}

/// The members of a settings object with nested objects folded into dotted
/// keys: `{"index":{"codec":"x"}}` reads as `index.codec`.
fn flat_members(members: &[(String, Json)], prefix: &str, out: &mut Vec<(String, Json)>) {
    for (key, value) in members {
        let name = if prefix.is_empty() {
            key.clone()
        } else {
            // ALLOC: one key a stored setting, bounded by the descriptor.
            format!("{prefix}.{key}")
        };
        match value {
            Json::Obj(inner) => flat_members(inner, &name, out),
            other => out.push((name, other.clone())),
        }
    }
}

fn value_json(text: &str) -> Json {
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        Json::Num(text.to_owned())
    } else {
        Json::Str(text.to_owned())
    }
}

/// The stored settings after `requested`, as JSON object text; `None` when
/// nothing changes.
fn apply(current: &[u8], requested: &[Setting], preserve: bool) -> Result<Option<String>, String> {
    let Json::Obj(top) = parse_json(current).map_err(|e| e.to_string())? else {
        return Err("the stored settings are not an object".to_owned());
    };
    // ALLOC: the stored settings, bounded by the descriptor.
    let mut held: Vec<(String, Json)> = Vec::new();
    flat_members(&top, "", &mut held);
    let before: Vec<Setting> = held
        .iter()
        .map(|(k, v)| (k.clone(), Some(text_of(v))))
        .collect();
    let Some(after) = merge(&before, requested, preserve) else {
        return Ok(None);
    };
    let mut out = String::from("{");
    for (i, (key, value)) in after.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        oss_doc::mapping::json::write_str(&mut out, key);
        out.push(':');
        let kept = held
            .iter()
            .find(|(k, v)| k == key && Some(text_of(v)) == *value)
            .map(|(_, v)| v.clone());
        let json = kept.unwrap_or_else(|| value_json(value.as_deref().unwrap_or("")));
        out.push_str(&json.to_json());
    }
    out.push('}');
    Ok(Some(out))
}

/// Serves the settings update route over the descriptors.
pub struct SettingsService {
    descriptors: DescriptorStore,
    buffer_bytes: usize,
    registry: Registry,
}

impl SettingsService {
    /// A service over `descriptors`; each request holds two buffers of
    /// `buffer_bytes`, charged to the request-body pool. `pipelines` says
    /// whether the node runs ingest pipelines.
    pub fn new(descriptors: DescriptorStore, buffer_bytes: usize, pipelines: bool) -> Self {
        Self {
            descriptors,
            buffer_bytes,
            registry: Registry { pipelines },
        }
    }

    fn update(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let index = req
            .path_param("index")
            .map(|n| n.into_owned())
            .ok_or_else(|| RestError::illegal_argument("an index is required"))?;
        let preserve = req.param("preserve_existing").as_deref() == Some("true");
        let body = read_body(
            req,
            MAX_BODY_BYTES,
            "the body of a settings update is larger than 1 MiB",
        )?;
        let requested =
            parse(Some(&body), &self.registry).map_err(|r| refuse(r.status, r.kind, r.reason))?;
        if let Some((key, _)) = requested.iter().find(|(k, _)| behaviour_changing(k)) {
            return Err(refuse(
                501,
                "unsupported_setting",
                // ALLOC: error path; one message.
                format!(
                    "{key} is not supported: it changes what a node does, and this node would store it and not act on it"
                ),
            ));
        }
        req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
        // ALLOC: two buffers a request, buffer_bytes each, reserved above.
        let mut read = vec![0u8; self.buffer_bytes];
        let mut write = vec![0u8; self.buffer_bytes];
        for _ in 0..ATTEMPTS {
            let held = self
                .descriptors
                .resolve(&index, &mut read)
                .map_err(|e| failed(&e.to_string()))?;
            let Target::Index(record) = held.target else {
                return Err(crate::get_route::index_not_found(&index));
            };
            let Some(text) =
                apply(record.settings, &requested, preserve).map_err(|e| failed(&e))?
            else {
                return resp.send(200, JSON, ACKNOWLEDGED.as_bytes());
            };
            let mut next = record.clone();
            next.settings = text.as_bytes();
            next.settings_version = next.settings_version.saturating_add(1);
            if self
                .descriptors
                .update(&next, held.generation, &mut write)
                .map_err(|e| failed(&e.to_string()))?
                .is_some()
            {
                return resp.send(200, JSON, ACKNOWLEDGED.as_bytes());
            }
        }
        Err(refuse(
            409,
            "version_conflict_engine_exception",
            // ALLOC: error path; one message.
            format!("the settings of [{index}] kept changing while this update was applied"),
        ))
    }
}

fn failed(reason: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(500, "settings_update_failure", reason.to_owned())
}

/// The settings update route: `PUT /{index}/_settings`.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<SettingsService>) -> Result<Router, RouteError> {
    let service = Arc::clone(service);
    router.route(
        Method::Put,
        "/{index}/_settings",
        PARAMS,
        move |req: &mut Request<'_>, resp: &mut Response<'_>| service.update(req, resp),
    )
}
