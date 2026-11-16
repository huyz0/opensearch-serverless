//! Every setting the node's modules define, collected once at start-up
//! (the shell's `ServerlessSettings`).

use std::fmt;

use super::{INDEX_PREFIX, Kind, NODE_PREFIX, Scope, Setting, SettingError};

/// Why a set of providers cannot form a registry: a definition is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryError(pub String);

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RegistryError {}

/// The node's settings, by key.
#[derive(Debug, Clone)]
pub struct Registry {
    /// Sorted by key.
    settings: Vec<&'static Setting>,
}

impl Registry {
    /// Collects every provider's settings: each provider is one module's
    /// `SETTINGS` list.
    ///
    /// # Errors
    ///
    /// [`RegistryError`] for a key defined twice, a key outside its scope's
    /// prefix ([`NODE_PREFIX`], [`INDEX_PREFIX`]), an empty doc, or a
    /// default the setting would refuse as a value.
    pub fn new(providers: &[&[&'static Setting]]) -> Result<Self, RegistryError> {
        // ALLOC: one entry a setting, once a node.
        let mut settings: Vec<&'static Setting> =
            providers.iter().flat_map(|p| p.iter().copied()).collect();
        for setting in &settings {
            check(setting)?;
        }
        settings.sort_by_key(|s| s.key);
        if let Some(pair) = settings
            .windows(2)
            .find(|w| w.first().map(|s| s.key) == w.get(1).map(|s| s.key))
        {
            let key = pair.first().map_or("", |s| s.key);
            // ALLOC: one message, only on a refusal.
            return Err(RegistryError(format!("setting [{key}] is defined twice")));
        }
        Ok(Self { settings })
    }

    /// The setting of `key`.
    pub fn get(&self, key: &str) -> Option<&'static Setting> {
        self.settings
            .binary_search_by(|s| s.key.cmp(key))
            .ok()
            .and_then(|at| self.settings.get(at).copied())
    }

    /// Every setting, by key.
    pub fn all(&self) -> impl Iterator<Item = &'static Setting> + '_ {
        self.settings.iter().copied()
    }

    /// The index-scoped settings, which an index's settings are checked
    /// against.
    pub fn index_scoped(&self) -> impl Iterator<Item = &'static Setting> + '_ {
        self.all().filter(|s| s.scope == Scope::Index)
    }

    /// The keys under [`NODE_PREFIX`] among `keys` that no setting defines,
    /// sorted: usually a typo, which would otherwise be ignored.
    pub fn unknown_node_keys<'k>(&self, keys: impl IntoIterator<Item = &'k str>) -> Vec<&'k str> {
        let mut unknown: Vec<&str> = keys
            .into_iter()
            .filter(|k| k.starts_with(NODE_PREFIX) && self.get(k).is_none())
            // ALLOC: one entry an unknown key, bounded by the keys given.
            .collect();
        unknown.sort_unstable();
        unknown.dedup();
        unknown
    }

    /// Checks one change to the cluster register's persistent settings:
    /// `value` for `key`, or `None` to remove it, as the Java's
    /// `ClusterSettings.validate`. A key outside [`NODE_PREFIX`] and
    /// [`INDEX_PREFIX`] passes: it is core's or a plugin's.
    ///
    /// # Errors
    ///
    /// [`SettingError`] with the Java's message for an unknown key under
    /// [`NODE_PREFIX`], any key under [`INDEX_PREFIX`] (an index setting is
    /// no cluster setting), a key that is not dynamic, or a value the
    /// setting refuses. The Java's "did you mean" suggestion is not made.
    pub fn check_update(&self, key: &str, value: Option<&str>) -> Result<(), SettingError> {
        let index_key = key.starts_with(INDEX_PREFIX);
        if !index_key && !key.starts_with(NODE_PREFIX) {
            return Ok(());
        }
        // ALLOC: each message below is one, only on a refusal.
        let Some(setting) = self.get(key).filter(|_| !index_key) else {
            return Err(SettingError::new(
                key,
                format!(
                    "unknown setting [{key}] please check that any required plugins are installed, \
                     or check the breaking changes documentation for removed settings"
                ),
            ));
        };
        if !setting.dynamic {
            return Err(SettingError::new(
                key,
                format!("persistent setting [{key}], not dynamically updateable"),
            ));
        }
        value.map_or(Ok(()), |v| setting.check(v))
    }
}

/// One definition's own rules.
fn check(setting: &Setting) -> Result<(), RegistryError> {
    let prefix = match setting.scope {
        Scope::Node => NODE_PREFIX,
        Scope::Index => INDEX_PREFIX,
    };
    let key = setting.key;
    // ALLOC: each message below is one, only on a refusal.
    if !key.starts_with(prefix) || key.len() == prefix.len() {
        return Err(RegistryError(format!(
            "setting [{key}] is not under [{prefix}]"
        )));
    }
    if setting.doc.trim().is_empty() {
        return Err(RegistryError(format!("setting [{key}] has no doc")));
    }
    let default_ok = match setting.kind {
        Kind::Bool { .. } | Kind::Str { .. } => true,
        Kind::Int { default, min, max } => min <= default && default <= max,
        Kind::Long { default, min, max } => min <= default && default <= max,
        Kind::Choice { default, choices } => choices.contains(&default),
    };
    if !default_ok {
        return Err(RegistryError(format!(
            "setting [{key}] has a default it would refuse as a value"
        )));
    }
    Ok(())
}
