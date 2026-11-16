//! A setting an index may override: the index's value, else the cluster
//! register's, else the node's, else the default.
//!
//! Ported from the shell's `LayeredSetting`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/settings/LayeredSetting.java@3920dc36`),
//! under two keys: the index's `index.serverless.<name>` and the cluster's
//! and node's `serverless.<name>`. An empty value is unset at every layer.
//! An index or cluster value that does not parse is skipped, so a bad
//! cluster value cannot stop an index or a node from deciding; a bad node
//! value is refused, as the Java refuses it when the node starts.

use std::convert::Infallible;

use super::{
    BoolSetting, ChoiceSetting, IntSetting, LongSetting, SettingError, Source, StrSetting,
};

/// Where a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// The index's settings.
    Index,
    /// The cluster register.
    Cluster,
    /// The node's own settings.
    Node,
    /// None set it.
    Default,
}

/// A resolved value and the layer it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved<T> {
    /// The value.
    pub value: T,
    /// Where it was set.
    pub layer: Layer,
}

/// The settings a value is resolved through, highest first.
#[derive(Clone, Copy)]
pub struct Layers<'a> {
    /// The index's settings, or `None` outside an index.
    pub index: Option<&'a dyn Source>,
    /// The cluster register's settings.
    pub cluster: &'a dyn Source,
    /// The node's settings.
    pub node: &'a dyn Source,
}

/// Resolves through `layers`: the index's value under `index_key` parsed
/// by `parse_index`, else the cluster's under `key` parsed by `parse`, each
/// skipped when it is empty or does not parse; else the node's, refused
/// when it does not parse; else `default`.
fn resolve<'a, T, E>(
    layers: &Layers<'a>,
    index_key: &str,
    key: &str,
    default: T,
    parse_index: impl Fn(&'a str) -> Result<T, E>,
    parse: impl Fn(&'a str) -> Result<T, E>,
) -> Result<Resolved<T>, E> {
    let set = |v: &&str| !v.is_empty();
    let index = layers.index.and_then(|i| i.value(index_key)).filter(set);
    if let Some(value) = index.and_then(|v| parse_index(v).ok()) {
        return Ok(Resolved {
            value,
            layer: Layer::Index,
        });
    }
    if let Some(value) = layers
        .cluster
        .value(key)
        .filter(set)
        .and_then(|v| parse(v).ok())
    {
        return Ok(Resolved {
            value,
            layer: Layer::Cluster,
        });
    }
    match layers.node.value(key).filter(set) {
        Some(v) => Ok(Resolved {
            value: parse(v)?,
            layer: Layer::Node,
        }),
        None => Ok(Resolved {
            value: default,
            layer: Layer::Default,
        }),
    }
}

impl BoolSetting {
    /// This node setting, overridden by `index` for an index.
    ///
    /// # Errors
    ///
    /// [`SettingError`] for a node value that does not parse; an index or
    /// cluster value that does not parse is skipped.
    pub fn resolve(
        &self,
        index: &BoolSetting,
        layers: &Layers<'_>,
    ) -> Result<Resolved<bool>, SettingError> {
        resolve(
            layers,
            index.key(),
            self.key(),
            self.default(),
            |v| index.parse(v),
            |v| self.parse(v),
        )
    }
}

impl IntSetting {
    /// This node setting, overridden by `index` for an index.
    ///
    /// # Errors
    ///
    /// As [`BoolSetting::resolve`].
    pub fn resolve(
        &self,
        index: &IntSetting,
        layers: &Layers<'_>,
    ) -> Result<Resolved<i32>, SettingError> {
        resolve(
            layers,
            index.key(),
            self.key(),
            self.default(),
            |v| index.parse(v),
            |v| self.parse(v),
        )
    }
}

impl LongSetting {
    /// This node setting, overridden by `index` for an index.
    ///
    /// # Errors
    ///
    /// As [`BoolSetting::resolve`].
    pub fn resolve(
        &self,
        index: &LongSetting,
        layers: &Layers<'_>,
    ) -> Result<Resolved<i64>, SettingError> {
        resolve(
            layers,
            index.key(),
            self.key(),
            self.default(),
            |v| index.parse(v),
            |v| self.parse(v),
        )
    }
}

impl ChoiceSetting {
    /// This node setting, overridden by `index` for an index.
    ///
    /// # Errors
    ///
    /// As [`BoolSetting::resolve`].
    pub fn resolve(
        &self,
        index: &ChoiceSetting,
        layers: &Layers<'_>,
    ) -> Result<Resolved<&'static str>, SettingError> {
        resolve(
            layers,
            index.key(),
            self.key(),
            self.default(),
            |v| index.parse(v),
            |v| self.parse(v),
        )
    }
}

impl StrSetting {
    /// This node setting, overridden by `index` for an index.
    pub fn resolve<'a>(&self, index: &StrSetting, layers: &Layers<'a>) -> Resolved<&'a str> {
        // A text value always parses, so no layer is ever refused.
        let Ok(resolved) =
            resolve::<_, Infallible>(layers, index.key(), self.key(), self.default(), Ok, Ok);
        resolved
    }
}
