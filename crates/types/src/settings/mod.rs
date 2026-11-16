//! Settings: each defined once, next to the code that reads it, with the
//! Java shell's key, type, default and messages.
//!
//! Ported from `server/src/main/java/org/opensearch/common/settings/Setting.java`
//! (`boolSetting`, `intSetting`, `longSetting`, `simpleString`, `parseInt`,
//! `parseLong`, `parseBoolean`) and the shell's `settings` package at
//! huyz0/OpenSearch@3920dc36. A module declares its settings as constants
//! and lists them in a `SETTINGS` slice, its provider; the node collects the
//! providers into a [`Registry`].
//!
//! A value is read from a [`Source`]: the node's settings, the cluster
//! register's, or an index's. [`Layers`] resolves a setting that an index
//! may override: the index's value, else the cluster's, else the node's,
//! else the default. Reading allocates nothing; only an error builds its
//! message.

mod layers;
mod live;
mod registry;
mod typed;

use std::collections::{BTreeMap, HashMap};
use std::fmt;

pub use layers::{Layer, Layers, Resolved};
pub use live::{Live, Snapshot};
pub use registry::{Registry, RegistryError};
pub use typed::{BoolSetting, ChoiceSetting, IntSetting, LongSetting, StrSetting};

/// The prefix of a node or cluster setting the shell owns.
pub const NODE_PREFIX: &str = "serverless.";

/// The prefix of an index setting the shell owns.
pub const INDEX_PREFIX: &str = "index.serverless.";

/// Where a setting is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// A node's settings, and the cluster register's (the Java's
    /// `NodeScope`).
    Node,
    /// An index's settings, in its descriptor (`IndexScope`).
    Index,
}

/// What a setting holds, with its default and bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `true` or `false`.
    Bool {
        /// The default.
        default: bool,
    },
    /// A 32-bit integer within `min..=max`.
    Int {
        /// The default.
        default: i32,
        /// The least value.
        min: i32,
        /// The greatest value.
        max: i32,
    },
    /// A 64-bit integer within `min..=max`.
    Long {
        /// The default.
        default: i64,
        /// The least value.
        min: i64,
        /// The greatest value.
        max: i64,
    },
    /// Any text.
    Str {
        /// The default; empty for none, as the Java's `simpleString`.
        default: &'static str,
    },
    /// One of a fixed list of words.
    Choice {
        /// The default, one of `choices`.
        default: &'static str,
        /// The words it may be.
        choices: &'static [&'static str],
    },
}

/// One setting's definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    /// The key, under [`NODE_PREFIX`] or [`INDEX_PREFIX`] by its scope.
    pub key: &'static str,
    /// Its type, default and bounds.
    pub kind: Kind,
    /// Where it is set.
    pub scope: Scope,
    /// Whether a running node reads it again: from the cluster register for
    /// a node setting, from the descriptor for an index one.
    pub dynamic: bool,
    /// What it does, for the operator.
    pub doc: &'static str,
}

impl Setting {
    /// Checks `value` as this setting would parse it.
    ///
    /// # Errors
    ///
    /// [`SettingError`] with the Java's message.
    pub fn check(&self, value: &str) -> Result<(), SettingError> {
        match self.kind {
            Kind::Bool { .. } => typed::parse_bool(self.key, value).map(drop),
            Kind::Int { min, max, .. } => typed::parse_int(self.key, value, min, max).map(drop),
            Kind::Long { min, max, .. } => typed::parse_long(self.key, value, min, max).map(drop),
            Kind::Str { .. } => Ok(()),
            Kind::Choice { choices, .. } => typed::parse_choice(self.key, value, choices).map(drop),
        }
    }
}

/// A setting value that does not parse, with the Java's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingError {
    /// The setting's key.
    pub key: String,
    /// What is wrong, as the Java says it.
    pub message: String,
}

impl SettingError {
    pub(crate) fn new(key: &str, message: String) -> Self {
        Self {
            // ALLOC: one key, only on a refusal.
            key: key.to_owned(),
            message,
        }
    }
}

impl fmt::Display for SettingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SettingError {}

/// Flat settings, by dotted key: the node's, the cluster register's, or an
/// index's.
pub trait Source {
    /// The value set for `key`, or `None` when it is not set.
    fn value(&self, key: &str) -> Option<&str>;
}

impl Source for BTreeMap<String, String> {
    fn value(&self, key: &str) -> Option<&str> {
        self.get(key).map(String::as_str)
    }
}

impl<S: std::hash::BuildHasher> Source for HashMap<String, String, S> {
    fn value(&self, key: &str) -> Option<&str> {
        self.get(key).map(String::as_str)
    }
}

impl Source for &[(&str, &str)] {
    fn value(&self, key: &str) -> Option<&str> {
        self.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }
}

/// No settings at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct Empty;

impl Source for Empty {
    fn value(&self, _: &str) -> Option<&str> {
        None
    }
}
