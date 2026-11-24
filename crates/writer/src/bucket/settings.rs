//! The node settings that turn buckets on and size them.
//!
//! Ported from the shell's `BucketSettings` (`ENABLED`, `MAX_MEMBERS`,
//! `TARGET_BYTES`, `MAX_MANIFEST_BYTES`, `MANIFEST_CODEC`, `limits`,
//! `manifestCodec` and `compressOnlyIfWorthIt`) at huyz0/OpenSearch
//! `429b4cbc`. Buckets are a switch, off until a measurement shows the
//! savings with no cost.

use oss_format::{MAX_BUCKET_MEMBERS, ManifestCodec};
use oss_types::settings::{
    BoolSetting, ChoiceSetting, IntSetting, LongSetting, Scope, Setting, SettingError, Source,
};

use super::{Limits, ManifestWrite};

/// `serverless.buckets.enabled`: whether new small indices are placed in
/// buckets. Default `false`.
pub const ENABLED: BoolSetting = BoolSetting::new(
    "serverless.buckets.enabled",
    false,
    Scope::Node,
    "Whether new small indices are placed in buckets that share one manifest register.",
);

/// `serverless.buckets.max_members`: the most indices one bucket holds, a
/// ceiling and not what fills a bucket. Above the manifest's own ceiling it
/// is clamped to it.
pub const MAX_MEMBERS: IntSetting = IntSetting::new(
    "serverless.buckets.max_members",
    1024,
    1,
    i32::MAX,
    Scope::Node,
    "The most indices one bucket holds; buckets fill by data size first.",
);

/// `serverless.buckets.target_bytes`: the data size at which a stripe stops
/// adding indices to its open bucket and opens the next. Default 1 GiB.
pub const TARGET_BYTES: LongSetting = LongSetting::new(
    "serverless.buckets.target_bytes",
    1 << 30,
    1,
    i64::MAX,
    Scope::Node,
    "The data size at which a stripe opens its next bucket.",
);

/// `serverless.buckets.max_manifest_bytes`: the stored manifest size past
/// which a bucket takes no new members. Default 2 MiB.
pub const MAX_MANIFEST_BYTES: IntSetting = IntSetting::new(
    "serverless.buckets.max_manifest_bytes",
    2 << 20,
    1,
    i32::MAX,
    Scope::Node,
    "The stored manifest size past which a bucket takes no new members.",
);

/// `serverless.buckets.manifest_codec`: how bucket manifests are written.
/// `auto` is the compact form compressed with zstd only when that pays,
/// `json` the form every node reads, the others the compact form always
/// compressed with that codec. Reading takes any, whatever this says.
pub const MANIFEST_CODEC: ChoiceSetting = ChoiceSetting::new(
    "serverless.buckets.manifest_codec",
    "auto",
    &["auto", "json", "none", "deflate", "zstd"],
    Scope::Node,
    "How bucket manifests are written: auto, json, or the codec none, deflate or zstd.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    ENABLED.setting(),
    MAX_MEMBERS.setting(),
    TARGET_BYTES.setting(),
    MAX_MANIFEST_BYTES.setting(),
    MANIFEST_CODEC.setting(),
];

/// Whether new small indices are placed in buckets.
///
/// # Errors
///
/// [`SettingError`] for a value that is no boolean.
pub fn enabled(source: &dyn Source) -> Result<bool, SettingError> {
    ENABLED.get(source)
}

/// The limits new members are placed by.
///
/// # Errors
///
/// [`SettingError`] for a size that is no number or is under one.
pub fn limits(source: &dyn Source) -> Result<Limits, SettingError> {
    let members = usize::try_from(MAX_MEMBERS.get(source)?).unwrap_or(MAX_BUCKET_MEMBERS);
    let target = u64::try_from(TARGET_BYTES.get(source)?).unwrap_or(1);
    let manifest = usize::try_from(MAX_MANIFEST_BYTES.get(source)?).unwrap_or(usize::MAX);
    Ok(Limits::new(target, members, manifest))
}

/// How a manifest is written.
///
/// # Errors
///
/// [`SettingError`] for a name that is none of the choices.
pub fn manifest_write(source: &dyn Source) -> Result<ManifestWrite, SettingError> {
    Ok(match MANIFEST_CODEC.get(source)? {
        "json" => ManifestWrite::Json,
        "none" => ManifestWrite::Compact(ManifestCodec::None),
        "deflate" => ManifestWrite::Compact(ManifestCodec::Deflate),
        "zstd" => ManifestWrite::Compact(ManifestCodec::Zstd),
        _ => ManifestWrite::Auto(ManifestCodec::Zstd),
    })
}
