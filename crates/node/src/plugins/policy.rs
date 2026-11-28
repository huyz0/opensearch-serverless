//! The plugin policy: one JSON document, the same on every node, naming each
//! plugin, the tier it is served from, what it is granted and the routes it
//! may claim (docs/specs/plugin-system AC-4, AC-5).

use oss_ext::Capability;
use serde_json::Value;

/// The most bytes a policy may be.
pub const MAX_POLICY_BYTES: usize = 256 * 1024;
/// The most plugins one policy may name.
pub const MAX_PLUGINS: usize = 64;
/// The most claims and the most action rows one plugin may have.
const MAX_LIST: usize = 64;
/// The longest plugin id.
const MAX_ID: usize = 41;

/// Which tier a plugin is served from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyTier {
    /// Code in this process.
    Native,
    /// A WASM module.
    Wasm,
    /// A separate process.
    Grpc,
}

/// One plugin's entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The plugin's id.
    pub id: String,
    /// The tier it is served from.
    pub tier: PolicyTier,
    /// For a linked-in plugin, the name it is registered under.
    pub name: String,
    /// What the node grants it.
    pub grants: Vec<Capability>,
    /// Path prefixes, outside `/_plugins/<id>/`, it may mount routes under.
    pub claims: Vec<String>,
    /// `"METHOD /pattern"` to the action that route is authorized as.
    pub actions: Vec<(String, String)>,
}

/// The parsed policy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Policy {
    entries: Vec<Entry>,
}

/// Why a policy was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyError(String);

impl PolicyError {
    fn at(key: &str, why: &str) -> Self {
        Self(format!("{key}: {why}"))
    }
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PolicyError {}

fn only_keys(
    obj: &serde_json::Map<String, Value>,
    at: &str,
    keys: &[&str],
) -> Result<(), PolicyError> {
    match obj.keys().find(|k| !keys.contains(&k.as_str())) {
        Some(k) => Err(PolicyError::at(&format!("{at}.{k}"), "unknown key")),
        None => Ok(()),
    }
}

fn strings(v: Option<&Value>, at: &str) -> Result<Vec<String>, PolicyError> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let items = v
        .as_array()
        .ok_or_else(|| PolicyError::at(at, "not an array"))?;
    if items.len() > MAX_LIST {
        return Err(PolicyError::at(at, "too many entries"));
    }
    items
        .iter()
        .map(|s| {
            s.as_str()
                .map(str::to_owned)
                .ok_or_else(|| PolicyError::at(at, "not a string"))
        })
        .collect()
}

fn entry(v: &Value, at: &str) -> Result<Entry, PolicyError> {
    let obj = v
        .as_object()
        .ok_or_else(|| PolicyError::at(at, "not an object"))?;
    only_keys(
        obj,
        at,
        &["id", "tier", "name", "grants", "claim", "actions"],
    )?;
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= MAX_ID)
        .ok_or_else(|| PolicyError::at(&format!("{at}.id"), "missing or too long"))?
        .to_owned();
    let tier = match obj.get("tier").and_then(Value::as_str) {
        Some("native") => PolicyTier::Native,
        Some("wasm") => PolicyTier::Wasm,
        Some("grpc") => PolicyTier::Grpc,
        _ => {
            return Err(PolicyError::at(
                &format!("{at}.tier"),
                "not native, wasm or grpc",
            ));
        }
    };
    let name = match obj.get("name") {
        None => id.clone(),
        Some(n) => n
            .as_str()
            .ok_or_else(|| PolicyError::at(&format!("{at}.name"), "not a string"))?
            .to_owned(),
    };
    let mut grants = Vec::new();
    for g in strings(obj.get("grants"), &format!("{at}.grants"))? {
        grants.push(
            Capability::parse(&g)
                .ok_or_else(|| PolicyError::at(&format!("{at}.grants"), "not a capability"))?,
        );
    }
    let claims = strings(obj.get("claim"), &format!("{at}.claim"))?;
    if claims
        .iter()
        .any(|c| !c.starts_with('/') || c.contains(['{', '}']))
    {
        return Err(PolicyError::at(&format!("{at}.claim"), "not a path prefix"));
    }
    let mut actions = Vec::new();
    if let Some(a) = obj.get("actions") {
        let a = a
            .as_object()
            .ok_or_else(|| PolicyError::at(&format!("{at}.actions"), "not an object"))?;
        if a.len() > MAX_LIST {
            return Err(PolicyError::at(
                &format!("{at}.actions"),
                "too many entries",
            ));
        }
        for (k, v) in a {
            let action = v
                .as_str()
                .ok_or_else(|| PolicyError::at(&format!("{at}.actions"), "not a string"))?;
            actions.push((k.clone(), action.to_owned()));
        }
    }
    Ok(Entry {
        id,
        tier,
        name,
        grants,
        claims,
        actions,
    })
}

impl Policy {
    /// The policy `text` states.
    ///
    /// # Errors
    ///
    /// The first key at fault.
    pub fn parse(text: &str) -> Result<Self, PolicyError> {
        if text.len() > MAX_POLICY_BYTES {
            return Err(PolicyError::at("policy", "too large"));
        }
        let v: Value =
            serde_json::from_str(text).map_err(|_| PolicyError::at("policy", "not JSON"))?;
        let obj = v
            .as_object()
            .ok_or_else(|| PolicyError::at("policy", "not an object"))?;
        only_keys(obj, "policy", &["plugins"])?;
        let list = match obj.get("plugins") {
            None => return Ok(Self::default()),
            Some(l) => l
                .as_array()
                .ok_or_else(|| PolicyError::at("plugins", "not an array"))?,
        };
        if list.len() > MAX_PLUGINS {
            return Err(PolicyError::at("plugins", "too many plugins"));
        }
        // ALLOC: at most MAX_PLUGINS entries, checked above.
        let mut entries: Vec<Entry> = Vec::new();
        for (i, v) in list.iter().enumerate() {
            let e = entry(v, &format!("plugins[{i}]"))?;
            if entries.iter().any(|o| o.id == e.id) {
                return Err(PolicyError::at(&format!("plugins[{i}].id"), "named twice"));
            }
            entries.push(e);
        }
        Ok(Self { entries })
    }

    /// The plugins it names, in order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
}
