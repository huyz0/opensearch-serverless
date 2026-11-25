//! What a settings update may change: the body's two shapes, the keys the
//! node's registry does not know, the asks this design refuses, and the
//! merge over the settings the descriptor already holds.

use std::collections::BTreeMap;

use oss_doc::mapping::json::{Json, parse as parse_json};

use super::flat::flatten;
use crate::route::resolve::Refusal;

/// How many times the descriptor swap is tried before the update is a conflict.
pub const ATTEMPTS: u32 = 4;

const SHARD_MODE_KEY: &str = "index.serverless.shard_mode";
const SHARDS_KEY: &str = "index.number_of_shards";
const REPLICAS_KEY: &str = "index.number_of_replicas";
const SEARCH_PIPELINE_KEY: &str = "index.search.default_pipeline";

/// What the node's registry says of one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A setting of this deployment's own, with the verdict of its own check.
    Ours(Result<(), String>),
    /// A setting core (or a plugin) declares, dynamic or not.
    Core {
        /// Whether it may change on a live index.
        dynamic: bool,
    },
    /// A key no registry declares.
    Unknown,
}

/// The node's index settings registry, as the update reads it.
pub trait Scope {
    /// What the registry says of `key`, given the value the request gives it.
    fn classify(&self, key: &str, value: Option<&str>) -> Kind;
}

/// One setting: its key and its value, none for a null.
pub type Setting = (String, Option<String>);

fn bad_request(kind: &'static str, reason: impl Into<String>) -> Refusal {
    Refusal::new(400, kind, reason)
}

fn unsupported(reason: impl Into<String>) -> Refusal {
    Refusal::new(501, "unsupported_setting", reason)
}

/// Reads a settings update body into normalised settings and checks them
/// against `scope`, before anything is written.
///
/// # Errors
///
/// A [`Refusal`] for a missing or malformed body, no settings, a key `scope`
/// does not know or whose own check fails, and each ask this design refuses.
pub fn parse(body: Option<&[u8]>, scope: &impl Scope) -> Result<Vec<Setting>, Refusal> {
    let body = body.filter(|b| !b.iter().all(u8::is_ascii_whitespace));
    let Some(body) = body else {
        return Err(bad_request("missing_body", "a settings body is required"));
    };
    let members = match parse_json(body) {
        Ok(Json::Obj(members)) => members,
        Ok(_) => {
            return Err(bad_request(
                "malformed_body",
                "could not parse the settings body: the body is not a JSON object",
            ));
        }
        Err(e) => {
            return Err(bad_request(
                "malformed_body",
                // ALLOC: one message, bounded by the parse error's text.
                format!("could not parse the settings body: {e}"),
            ));
        }
    };
    let source = match members.iter().find(|(key, _)| key == "settings") {
        Some((_, Json::Obj(inner))) => inner.as_slice(),
        _ => members.as_slice(),
    };
    let requested = flatten(source);
    if requested.is_empty() {
        return Err(bad_request("missing_body", "no settings were given"));
    }
    analysis_is_final(&requested)?;
    unknown(&requested, scope)?;
    refuse(&requested, scope)?;
    Ok(requested)
}

/// A pinned analyzer is set at create and never changed
/// ([ADR 0053](../../../../docs/adr/0053-the-analyzer-extension-point.md)),
/// as stock OpenSearch's `index.analysis.*` is final on an open index.
fn analysis_is_final(requested: &[Setting]) -> Result<(), Refusal> {
    match requested
        .iter()
        .find(|(key, _)| key.starts_with("index.analysis.analyzer.ext_"))
    {
        None => Ok(()),
        Some((key, _)) => Err(bad_request(
            "illegal_argument_exception",
            // ALLOC: one message, bounded by the key.
            format!("final index setting [{key}], not updateable"),
        )),
    }
}

/// `[a, b]`, the way a sorted set prints.
fn listed(keys: &[&str]) -> String {
    // ALLOC: one list of the keys named, bounded by the body.
    format!("[{}]", keys.join(", "))
}

fn plural(count: usize) -> &'static str {
    if count > 1 { "s " } else { " " }
}

fn unknown(requested: &[Setting], scope: &impl Scope) -> Result<(), Refusal> {
    // ALLOC: the unknown keys, bounded by the body.
    let mut unknown: Vec<&str> = Vec::default();
    for (key, value) in requested {
        match scope.classify(key, value.as_deref()) {
            Kind::Ours(Err(reason)) => {
                return Err(bad_request("illegal_argument_exception", reason));
            }
            Kind::Unknown if key != SHARD_MODE_KEY => unknown.push(key),
            _ => {}
        }
    }
    if unknown.is_empty() {
        return Ok(());
    }
    Err(bad_request(
        "illegal_argument_exception",
        // ALLOC: one message, bounded by the keys named.
        format!(
            "unknown index setting{}{}",
            plural(unknown.len()),
            listed(&unknown)
        ),
    ))
}

fn held<'a>(settings: &'a [Setting], key: &str) -> Option<&'a str> {
    settings
        .iter()
        .find(|(name, _)| name == key)
        .and_then(|(_, value)| value.as_deref())
}

fn refuse(requested: &[Setting], scope: &impl Scope) -> Result<(), Refusal> {
    if held(requested, SHARD_MODE_KEY).is_some() {
        return Err(unsupported(
            "index.serverless.shard_mode is chosen when an index is created and cannot be changed: it decides how the index's units \
start, and the units an index already has stay as they are",
        ));
    }
    if held(requested, SHARDS_KEY).is_some() {
        return Err(unsupported(
            "number_of_shards cannot be changed after an index is created: the shard list lives in the \
index descriptor and routing is a function of it, so changing the count would re-route every \
document that already exists. An index that needs a different shard count is a new index",
        ));
    }
    if held(requested, REPLICAS_KEY).is_some_and(|replicas| replicas != "0") {
        return Err(unsupported(
            "number_of_replicas is not supported: a shard has one writer and its durability comes from \
the object store, not from replica copies. Readers are added by scaling search nodes, not \
by setting a replica count",
        ));
    }
    // A registry that owns the key (search pipelines on) checks it like any other of its own.
    if held(requested, SEARCH_PIPELINE_KEY).is_some_and(|v| {
        matches!(
            scope.classify(SEARCH_PIPELINE_KEY, Some(v)),
            Kind::Core { .. }
        )
    }) {
        return Err(unsupported(
            "index.search.default_pipeline is not supported: an index-level search pipeline is applied by the search path, \
and this shell's does not read it, so the value would be stored and never run. Name the pipeline on \
the request instead (?search_pipeline=)",
        ));
    }
    // ALLOC: the static keys, bounded by the body.
    let mut statics: Vec<&str> = Vec::default();
    for (key, value) in requested {
        let is_static = matches!(
            scope.classify(key, value.as_deref()),
            Kind::Core { dynamic: false }
        );
        if key != REPLICAS_KEY && is_static {
            statics.push(key);
        }
    }
    if statics.is_empty() {
        return Ok(());
    }
    Err(unsupported(
        // ALLOC: one message, bounded by the keys named.
        format!(
            "static index setting{}{} cannot be changed on a live index. Classic OpenSearch changes these \
only while an index is closed, and this design has no closed state: an index is either being served or \
it does not exist. Set them at creation",
            plural(statics.len()),
            listed(&statics)
        ),
    ))
}

/// The settings after `requested` is layered over `current`: a requested key
/// replaces the current value, a null removes the key, with `preserve_existing`
/// a key `current` already holds a value for keeps it, and the replica count
/// never stays. Sorted by key; none when nothing changed.
pub fn merge(
    current: &[Setting],
    requested: &[Setting],
    preserve_existing: bool,
) -> Option<Vec<Setting>> {
    // ALLOC: the current settings, bounded by the descriptor's.
    let before: BTreeMap<&str, &Option<String>> =
        current.iter().map(|(k, v)| (k.as_str(), v)).collect();
    // ALLOC: the merged settings, bounded by the descriptor's and the body's.
    let mut merged: BTreeMap<String, Option<String>> = current.iter().cloned().collect();
    for (key, value) in requested {
        if preserve_existing && held(current, key).is_some() {
            continue;
        }
        match value {
            Some(_) => {
                merged.insert(key.clone(), value.clone());
            }
            None => {
                merged.remove(key);
            }
        }
    }
    merged.remove(REPLICAS_KEY);
    let same = merged.len() == before.len()
        && merged
            .iter()
            .all(|(k, v)| before.get(k.as_str()).is_some_and(|b| *b == v));
    if same {
        return None;
    }
    // ALLOC: the merged settings as a list.
    Some(merged.into_iter().collect())
}
