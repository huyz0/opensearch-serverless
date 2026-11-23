//! Whether an index is grouped into a bucket, and with what.
//!
//! Ported from the shell's `BucketPolicy` and the policy and group settings
//! of `BucketSettings` (huyz0/OpenSearch `feature/serverlessshell`). One
//! setting, per index or for the cluster with the index's winning, says
//! `auto` (group while small) or `off`; unset everywhere, the node's
//! `serverless.buckets.enabled` decides. It only decides placement: which
//! head owns an index follows its descriptor on every node, so changing it
//! never splits ownership.

use oss_types::settings::{
    ChoiceSetting, Layers, Scope, Setting, SettingError, Source, StrSetting,
};

use super::settings::ENABLED;

/// `index.serverless.bucket`: `auto` or `off` for one index; unset by
/// default.
pub const INDEX_POLICY: StrSetting = StrSetting::new(
    "index.serverless.bucket",
    "",
    Scope::Index,
    "Whether this index is grouped into a bucket while small: auto or off.",
);

/// `serverless.bucket`: `auto` or `off` for every index that sets none, in
/// the cluster register; unset by default.
pub const CLUSTER_POLICY: StrSetting = StrSetting::new(
    "serverless.bucket",
    "",
    Scope::Node,
    "Whether indices are grouped into buckets while small: auto or off.",
);

/// `index.serverless.bucket_group`: the group an index is bucketed with,
/// such as its tenant; the index name when unset.
pub const GROUP: StrSetting = StrSetting::new(
    "index.serverless.bucket_group",
    "",
    Scope::Index,
    "The group this index is bucketed with, such as its tenant; the index name by default.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    INDEX_POLICY.setting(),
    CLUSTER_POLICY.setting(),
    GROUP.setting(),
];

/// The words a policy takes; its refusal is this setting's.
const WORDS: ChoiceSetting = ChoiceSetting::new(
    "index.serverless.bucket",
    "auto",
    &["auto", "off"],
    Scope::Index,
    "",
);

/// A value as `Some(true)` for `auto`, `Some(false)` for `off`, and `None`
/// for unset. Letter case is ignored.
fn parse(value: &str) -> Result<Option<bool>, SettingError> {
    if value.is_empty() {
        return Ok(None);
    }
    if value.eq_ignore_ascii_case("auto") {
        return Ok(Some(true));
    }
    if value.eq_ignore_ascii_case("off") {
        return Ok(Some(false));
    }
    // Not a policy: the choice names the words it may be.
    WORDS.parse(value).map(|_| None)
}

/// Checks a value given for either policy setting.
///
/// # Errors
///
/// [`SettingError`] for a value that is neither `auto`, `off` nor empty.
pub fn check(value: &str) -> Result<(), SettingError> {
    parse(value).map(|_| ())
}

/// Whether an index is grouped into a bucket, when it is small enough to be.
///
/// The index's policy decides, else the cluster's, else the node's
/// `serverless.buckets.enabled`. An empty value is unset at its layer, and an
/// index or cluster value that is no policy is skipped, so a bad cluster
/// value cannot stop an index or a node from deciding.
///
/// # Errors
///
/// [`SettingError`] for a node value that is no boolean, read only when
/// neither the index nor the cluster decided.
pub fn wanted(layers: &Layers<'_>) -> Result<bool, SettingError> {
    let decided =
        |source: &dyn Source, setting: &StrSetting| parse(setting.get(source)).ok().flatten();
    if let Some(index) = layers.index
        && let Some(want) = decided(index, &INDEX_POLICY)
    {
        return Ok(want);
    }
    if let Some(want) = decided(layers.cluster, &CLUSTER_POLICY) {
        return Ok(want);
    }
    ENABLED.get(layers.node)
}

/// The group an index is bucketed with: its group setting, else its name.
#[must_use]
pub fn group<'a>(index_name: &'a str, index: Option<&'a dyn Source>) -> &'a str {
    match index.map(|source| GROUP.get(source)) {
        Some(group) if !group.is_empty() => group,
        _ => index_name,
    }
}
