//! Which object store a node runs on, from its `serverless.store.*`
//! settings.
//!
//! Ported from the shell's `ObjectStoreSettings` and `ObjectStores.create`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/store/ObjectStores.java@3920dc36`):
//! the same keys, defaults and refusals. Credentials are not settings here
//! either; the Java reads them from the node's keystore.

use std::path::PathBuf;

use oss_types::settings::{BoolSetting, ChoiceSetting, Scope, Setting, Source, StrSetting};

/// `serverless.store.type`: `fs` (the default) or `s3`.
pub const TYPE: ChoiceSetting = ChoiceSetting::new(
    "serverless.store.type",
    "fs",
    &["fs", "s3"],
    Scope::Node,
    "The object store the node runs on: `fs`, a directory, or `s3`, a bucket.",
);
/// `serverless.store.path`: the directory of an `fs` store. Required for it.
pub const PATH: StrSetting = StrSetting::new(
    "serverless.store.path",
    "",
    Scope::Node,
    "The directory of an `fs` store; required for one.",
);
/// `serverless.store.bucket`: the bucket of an `s3` store. Required for it.
pub const BUCKET: StrSetting = StrSetting::new(
    "serverless.store.bucket",
    "",
    Scope::Node,
    "The bucket of an `s3` store; required for one.",
);
/// `serverless.store.region`: the bucket's region, default `us-east-1`.
pub const REGION: StrSetting = StrSetting::new(
    "serverless.store.region",
    "us-east-1",
    Scope::Node,
    "The region of an `s3` store's bucket.",
);
/// `serverless.store.endpoint`: an S3-compatible endpoint; AWS's when absent.
pub const ENDPOINT: StrSetting = StrSetting::new(
    "serverless.store.endpoint",
    "",
    Scope::Node,
    "An S3-compatible endpoint for an `s3` store; AWS's when unset.",
);
/// `serverless.store.path_style_access`: address the bucket by path, as
/// most S3-compatible services need. Default `false`.
pub const PATH_STYLE_ACCESS: BoolSetting = BoolSetting::new(
    "serverless.store.path_style_access",
    false,
    Scope::Node,
    "Address an `s3` store's bucket by path, as most S3-compatible services need.",
);

/// Every setting this crate defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    TYPE.setting(),
    PATH.setting(),
    BUCKET.setting(),
    REGION.setting(),
    ENDPOINT.setting(),
    PATH_STYLE_ACCESS.setting(),
];

/// The store a node's settings select.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreSettings {
    /// A directory on this host.
    Fs {
        /// The directory.
        path: PathBuf,
    },
    /// An S3 bucket.
    S3 {
        /// The bucket.
        bucket: String,
        /// Its region.
        region: String,
        /// The endpoint, if not AWS's.
        endpoint: Option<String>,
        /// Whether the bucket is addressed by path.
        path_style_access: bool,
    },
}

impl StoreSettings {
    /// Reads the store settings from `source`. The error is the Java's
    /// message.
    pub fn parse(source: &dyn Source) -> Result<Self, String> {
        // ALLOC: each String below is one setting value or one error message,
        // once per node start; never on a request path.
        let present = |s: &StrSetting| source.value(s.key()).filter(|v| !v.trim().is_empty());
        // The Java's own message, not the choice's: the type is read as text.
        match source.value(TYPE.key()).unwrap_or(TYPE.default()) {
            "fs" => {
                let path = present(&PATH)
                    // ALLOC: one setting value or message, once per node start.
                    .ok_or_else(|| format!("{} is required for a filesystem store", PATH.key()))?;
                Ok(StoreSettings::Fs {
                    path: PathBuf::from(path),
                })
            }
            "s3" => {
                let bucket = present(&BUCKET)
                    // ALLOC: one setting value or message, once per node start.
                    .ok_or_else(|| format!("{} is required for an s3 store", BUCKET.key()))?;
                let path_style_access = PATH_STYLE_ACCESS.get(source).map_err(|e| e.message)?;
                let endpoint = source.value(ENDPOINT.key());
                Ok(StoreSettings::S3 {
                    // ALLOC: one setting value or message, once per node start.
                    bucket: bucket.to_owned(),
                    // ALLOC: one setting value or message, once per node start.
                    region: REGION.get(source).to_owned(),
                    // ALLOC: one setting value or message, once per node start.
                    endpoint: endpoint.map(str::to_owned),
                    path_style_access,
                })
            }
            // ALLOC: one setting value or message, once per node start.
            other => Err(format!(
                "unknown {}: {other}; expected fs or s3",
                TYPE.key()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(pairs: &[(&str, &'static str)]) -> Result<StoreSettings, String> {
        StoreSettings::parse(&pairs)
    }

    #[test]
    fn fs_is_the_default_and_needs_a_path() {
        assert_eq!(
            parse(&[(PATH.key(), "/data/store")]),
            Ok(StoreSettings::Fs {
                path: PathBuf::from("/data/store")
            })
        );
        assert_eq!(
            parse(&[(TYPE.key(), "fs"), (PATH.key(), "d")]),
            Ok(StoreSettings::Fs {
                path: PathBuf::from("d")
            })
        );
        let missing = Err("serverless.store.path is required for a filesystem store".to_owned());
        assert_eq!(parse(&[]), missing);
        assert_eq!(parse(&[(TYPE.key(), "fs"), (PATH.key(), "  ")]), missing);
    }

    #[test]
    fn s3_needs_a_bucket_and_defaults_the_rest() {
        assert_eq!(
            parse(&[(TYPE.key(), "s3"), (BUCKET.key(), "b")]),
            Ok(StoreSettings::S3 {
                bucket: "b".to_owned(),
                region: "us-east-1".to_owned(),
                endpoint: None,
                path_style_access: false,
            })
        );
        assert_eq!(
            parse(&[
                (TYPE.key(), "s3"),
                (BUCKET.key(), "b"),
                (REGION.key(), "eu-west-1"),
                (ENDPOINT.key(), "http://127.0.0.1:9000"),
                (PATH_STYLE_ACCESS.key(), "true"),
            ]),
            Ok(StoreSettings::S3 {
                bucket: "b".to_owned(),
                region: "eu-west-1".to_owned(),
                endpoint: Some("http://127.0.0.1:9000".to_owned()),
                path_style_access: true,
            })
        );
        assert_eq!(
            parse(&[(TYPE.key(), "s3")]),
            Err("serverless.store.bucket is required for an s3 store".to_owned())
        );
        assert_eq!(
            parse(&[
                (TYPE.key(), "s3"),
                (BUCKET.key(), "b"),
                (PATH_STYLE_ACCESS.key(), "yes")
            ]),
            Err("Failed to parse value [yes] as only [true] or [false] are allowed.".to_owned())
        );
    }

    #[test]
    fn another_type_is_refused() {
        assert_eq!(
            parse(&[(TYPE.key(), "gcs")]),
            Err("unknown serverless.store.type: gcs; expected fs or s3".to_owned())
        );
    }
}
