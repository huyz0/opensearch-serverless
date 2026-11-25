//! `PUT /{index}/_settings`: change the settings of one index.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/indexadmin/SettingsUpdateHandler.java`.
//! The decisions (body shapes, unknown keys, refusals, merge) are
//! `oss-coordinator`'s `indices::settings_update`; this route classifies each
//! key against the node's registry, swaps the descriptor with the merged
//! settings and bumps `settings_version`.

use std::collections::HashSet;
use std::sync::Arc;

use oss_coordinator::indices::settings_update::{ATTEMPTS, Kind, Scope, Setting, merge, parse};
use oss_doc::mapping::json::{Json, parse as parse_json};
use oss_meta::{DescriptorStore, MetaError, Target};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::metadata_block;
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

/// The most indices one update may name by pattern.
const PATTERN_CAP: usize = 1_000;

const ACKNOWLEDGED: &str = "{\"acknowledged\":true}";

/// The most hits a search window may be set to: a search's hits are held in
/// memory, so the node bounds the window (the reader's `MAX_RESULT_WINDOW`).
const WINDOW_CEILING: u32 = 10_000;

const WINDOW_KEY: &str = "index.max_result_window";

/// The block settings: the write blocks a bulk reads from the descriptor, the
/// read block a reader opens with, and the metadata block this route checks.
fn write_block_key(key: &str) -> bool {
    matches!(
        key,
        "index.blocks.write"
            | "index.blocks.read_only"
            | "index.blocks.read_only_allow_delete"
            | "index.blocks.read"
            | "index.blocks.metadata"
    )
}

/// What this node's registry says of an index setting.
struct Registry {
    pipelines: bool,
    search_pipelines: bool,
}

impl Scope for Registry {
    fn classify(&self, key: &str, value: Option<&str>) -> Kind {
        // Core declares these; the update's own refusals answer for them.
        if matches!(
            key,
            "index.number_of_shards" | "index.number_of_replicas" | "index.serverless.shard_mode"
        ) || (key == "index.search.default_pipeline" && !self.search_pipelines)
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
        if key == WINDOW_KEY {
            return Kind::Ours(match value.map(str::trim) {
                None => Ok(()),
                Some(text) => match text.parse::<u32>() {
                    Ok(w) if (1..=WINDOW_CEILING).contains(&w) => Ok(()),
                    Ok(w) if w > WINDOW_CEILING => Err(format!(
                        "[{key}] of [{w}] is above the most this node holds, [{WINDOW_CEILING}]: a search's hits are kept in memory"
                    )),
                    _ => Err(format!(
                        "failed to parse value [{text}] for setting [{key}], must be >= 1"
                    )),
                },
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
            crate::create_settings::checked(&pair, 1, self.pipelines, self.search_pipelines)
                .map(|_| ())
                .map_err(|e| {
                    e.as_exception()
                        .map_or_else(String::new, |x| x.reason().to_owned())
                }),
        )
    }
}

/// The refusal of a settings change to an index with a metadata block,
/// unless the change sets only block settings (so the block can be lifted).
fn metadata_blocked(index: &str, settings: &[u8], requested: &[Setting]) -> Result<(), RestError> {
    if requested
        .iter()
        .all(|(k, _)| k.starts_with("index.blocks."))
    {
        return Ok(());
    }
    metadata_block::check_settings(index, settings)
}

/// A setting's value as the text a request gives.
pub(crate) fn text_of(value: &Json) -> String {
    match value {
        Json::Str(s) | Json::Num(s) => s.clone(),
        Json::Bool(b) => b.to_string(),
        other => other.to_json(),
    }
}

/// The members of a settings object with nested objects folded into dotted
/// keys: `{"index":{"codec":"x"}}` reads as `index.codec`.
pub(crate) fn flat_members(
    members: &[(String, Json)],
    prefix: &str,
    out: &mut Vec<(String, Json)>,
) {
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
    pattern_cap: usize,
}

impl SettingsService {
    /// A service over `descriptors`; each request holds two buffers of
    /// `buffer_bytes`, charged to the request-body pool. `pipelines` says
    /// whether the node runs ingest pipelines.
    pub fn new(descriptors: DescriptorStore, buffer_bytes: usize, pipelines: bool) -> Self {
        Self {
            descriptors,
            buffer_bytes,
            registry: Registry {
                pipelines,
                search_pipelines: false,
            },
            pattern_cap: PATTERN_CAP,
        }
    }

    /// Whether `index.search.default_pipeline` is accepted: on when the node
    /// runs search pipelines.
    #[must_use]
    pub fn with_search_pipelines(mut self, on: bool) -> Self {
        self.registry.search_pipelines = on;
        self
    }

    /// Lowers the most indices a pattern may match.
    #[must_use]
    pub fn with_pattern_cap(mut self, cap: usize) -> Self {
        self.pattern_cap = cap;
        self
    }

    fn update(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let expression = req
            .path_param("index")
            .map_or_else(|| "_all".to_owned(), |n| n.into_owned());
        let preserve = req.param("preserve_existing").as_deref() == Some("true");
        let body = read_body(
            req,
            MAX_BODY_BYTES,
            "the body of a settings update is larger than 1 MiB",
        )?;
        let requested =
            parse(Some(&body), &self.registry).map_err(|r| refuse(r.status, r.kind, r.reason))?;
        req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
        // ALLOC: two buffers a request, buffer_bytes each, reserved above.
        let mut read = vec![0u8; self.buffer_bytes];
        let mut write = vec![0u8; self.buffer_bytes];
        // Every name is resolved before the first write, so a name that is
        // no index refuses the whole request.
        let targets = self.targets(&expression, &mut read)?;
        // A metadata block refuses the whole request before the first write.
        if !requested
            .iter()
            .all(|(k, _)| k.starts_with("index.blocks."))
        {
            for index in &targets {
                let held = self
                    .descriptors
                    .resolve(index, &mut read)
                    .map_err(|e| failed(&e.to_string()))?;
                if let Target::Index(record) = held.target {
                    metadata_blocked(index, record.settings, &requested)?;
                }
            }
        }
        for index in &targets {
            self.update_one(index, &requested, preserve, &mut read, &mut write)?;
        }
        resp.send(200, JSON, ACKNOWLEDGED.as_bytes())
    }

    /// The indices `expression` stands for: names, trailing-`*` prefix
    /// patterns, `_all` and aliases, each once, in order.
    fn targets(&self, expression: &str, buf: &mut [u8]) -> Result<Vec<String>, RestError> {
        // ALLOC: one entry a name or match, bounded by the expression and the cap a pattern.
        let mut order: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut add = |name: String, order: &mut Vec<String>| {
            if seen.insert(name.clone()) {
                order.push(name);
            }
        };
        for name in expression.split(',').map(str::trim) {
            if name.is_empty() {
                return Err(RestError::illegal_argument("an empty index name was given"));
            }
            let name = if name == "_all" { "*" } else { name };
            if let Some(prefix) = name.strip_suffix('*')
                && !prefix.contains(['*', '?'])
            {
                let matched = self
                    .descriptors
                    .names_with_prefix(prefix, self.pattern_cap)
                    .map_err(|e| match e {
                        MetaError::TooManyMatches { .. } => refuse(
                            400,
                            "too_many_indices",
                            // ALLOC: error path; one message.
                            format!("[{name}] matches more than {} indices", self.pattern_cap),
                        ),
                        other => failed(&other.to_string()),
                    })?;
                for m in matched {
                    // A pattern filters what exists: an alias or a tombstone is no index.
                    let held = self
                        .descriptors
                        .resolve(&m, buf)
                        .map_err(|e| failed(&e.to_string()))?;
                    if matches!(held.target, Target::Index(_)) {
                        add(m, &mut order);
                    }
                }
            } else if name.contains(['*', '?']) {
                return Err(refuse(
                    501,
                    "unsupported_setting",
                    // ALLOC: error path; one message.
                    format!(
                        "only a trailing-wildcard pattern is supported, and [{name}] is not one"
                    ),
                ));
            } else {
                let held = self
                    .descriptors
                    .resolve(name, buf)
                    .map_err(|e| failed(&e.to_string()))?;
                match held.target {
                    Target::Index(_) => add(name.to_owned(), &mut order),
                    Target::Alias(alias) => {
                        for member in &alias.indices {
                            add(member.to_string(), &mut order);
                        }
                    }
                    Target::Absent => return Err(crate::get_route::index_not_found(name)),
                }
            }
        }
        Ok(order)
    }

    /// One index: the merged settings swapped in, retried against a
    /// generation that moved.
    fn update_one(
        &self,
        index: &str,
        requested: &[Setting],
        preserve: bool,
        read: &mut [u8],
        write: &mut [u8],
    ) -> Result<(), RestError> {
        for _ in 0..ATTEMPTS {
            let held = self
                .descriptors
                .resolve(index, read)
                .map_err(|e| failed(&e.to_string()))?;
            let Target::Index(record) = held.target else {
                return Err(crate::get_route::index_not_found(index));
            };
            let Some(text) = apply(record.settings, requested, preserve).map_err(|e| failed(&e))?
            else {
                return Ok(());
            };
            let mut next = record.clone();
            next.settings = text.as_bytes();
            next.settings_version = next.settings_version.saturating_add(1);
            if self
                .descriptors
                .update(&next, held.generation, write)
                .map_err(|e| failed(&e.to_string()))?
                .is_some()
            {
                return Ok(());
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

/// The settings update routes: `PUT /{index}/_settings` and `PUT /_settings`.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<SettingsService>) -> Result<Router, RouteError> {
    let all = Arc::clone(service);
    let service = Arc::clone(service);
    router
        .route(
            Method::Put,
            "/{index}/_settings",
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.update(req, resp),
        )?
        .route(
            Method::Put,
            "/_settings",
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| all.update(req, resp),
        )
}
