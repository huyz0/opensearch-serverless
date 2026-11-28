//! The remote settings.
use std::collections::BTreeMap;

use oss_blob::{RootError, SourceRoots};

/// The most remotes one settings body may name.
pub const MAX_REMOTES: usize = 16;

/// Where a remote's store is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    /// An S3 bucket and key prefix.
    S3 {
        /// The bucket.
        bucket: String,
        /// The prefix inside it; empty for the whole bucket.
        prefix: String,
    },
    /// A directory.
    Fs {
        /// The directory.
        root: String,
    },
}

/// A named leader store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    /// The remote's name.
    pub alias: String,
    /// Where its store is.
    pub location: Location,
}

/// Why a remote setting is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsError(pub String);

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SettingsError {}

/// The remotes a flat settings update names.
///
/// # Errors
///
/// A [`SettingsError`].
pub fn remotes(settings: &[(String, Option<String>)]) -> Result<Vec<Remote>, SettingsError> {
    // ALLOC: one entry a remote of the body, which the edge bounds.
    let mut found: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (key, value) in settings {
        let Some(rest) = key.strip_prefix(PREFIX) else {
            continue;
        };
        let Some((alias, field)) = rest.split_once('.') else {
            continue;
        };
        let Some(field) = field.strip_prefix("store.") else {
            continue;
        };
        // A removal names no store to check.
        if let Some(value) = value {
            // ALLOC: one pair a store field of the body.
            found.entry(alias).or_default().push((field, value));
        }
    }
    if found.len() > MAX_REMOTES {
        return Err(SettingsError(format!(
            "at most {MAX_REMOTES} remotes may be set in one update, got {}",
            found.len()
        )));
    }
    found
        .into_iter()
        .map(|(alias, fields)| remote(alias, &fields))
        .collect()
}

const PREFIX: &str = "cluster.remote.";

fn refuse(message: String) -> SettingsError {
    SettingsError(message)
}

fn valid_alias(alias: &str) -> bool {
    let mut chars = alias.chars();
    alias.len() <= 64
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn remote(alias: &str, fields: &[(&str, &str)]) -> Result<Remote, SettingsError> {
    if !valid_alias(alias) {
        return Err(refuse(format!(
            "[{alias}] is not a valid remote name: use lower-case letters, digits, '_' and '-', at most 64, starting with a letter or digit"
        )));
    }
    let get = |name: &str| {
        fields
            .iter()
            .rev()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
    };
    if let Some((unknown, _)) = fields
        .iter()
        .find(|(k, _)| !matches!(*k, "type" | "bucket" | "prefix" | "root" | "profile"))
    {
        return Err(refuse(format!(
            "[cluster.remote.{alias}.store.{unknown}] is not a remote store setting"
        )));
    }
    if get("profile").is_some() {
        return Err(refuse(format!(
            "[cluster.remote.{alias}.store.profile]: credential profiles are not supported; the node's own read credentials are used"
        )));
    }
    let location = match get("type") {
        Some("s3") => {
            let bucket = get("bucket").filter(|b| !b.is_empty()).ok_or_else(|| {
                refuse(format!(
                    "[cluster.remote.{alias}.store.bucket] is required for an s3 store"
                ))
            })?;
            Location::S3 {
                bucket: bucket.to_owned(),
                prefix: get("prefix").unwrap_or_default().to_owned(),
            }
        }
        Some("fs") => {
            let root = get("root").filter(|r| !r.is_empty()).ok_or_else(|| {
                refuse(format!(
                    "[cluster.remote.{alias}.store.root] is required for an fs store"
                ))
            })?;
            Location::Fs {
                root: root.to_owned(),
            }
        }
        Some(other) => {
            return Err(refuse(format!(
                "[cluster.remote.{alias}.store.type] must be [s3] or [fs], got [{other}]"
            )));
        }
        None => {
            return Err(refuse(format!(
                "[cluster.remote.{alias}.store.type] is required: [s3] or [fs]"
            )));
        }
    };
    Ok(Remote {
        alias: alias.to_owned(),
        location,
    })
}

impl Remote {
    /// Checks the location against the operator's roots and this deployment's own store.
    ///
    /// # Errors
    ///
    /// A [`SettingsError`] naming the rule.
    pub fn check_roots(
        &self,
        roots: &SourceRoots,
        own: Option<(&str, &str)>,
    ) -> Result<(), SettingsError> {
        let alias = &self.alias;
        let fail = |e: RootError| {
            SettingsError(format!(
                "[cluster.remote.{alias}.store] is refused: {e} (OSS_REPLICATION_REMOTE_ROOTS)"
            ))
        };
        match &self.location {
            Location::Fs { root } => roots.fs_location(root).map(|_| ()).map_err(fail),
            Location::S3 { bucket, prefix } => {
                let (own_bucket, own_prefix) = own.ok_or_else(|| {
                    SettingsError(format!(
                        "[cluster.remote.{alias}.store] is refused: this node's own store is not S3, so it reads no s3 remote"
                    ))
                })?;
                roots
                    .s3_prefix(bucket, prefix, own_bucket, own_prefix)
                    .map(|_| ())
                    .map_err(fail)
            }
        }
    }
}
