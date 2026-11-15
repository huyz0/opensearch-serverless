//! Which object store a node runs on, from its `serverless.store.*`
//! settings.
//!
//! Ported from the shell's `ObjectStoreSettings` and `ObjectStores.create`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/store/ObjectStores.java@3920dc36`):
//! the same keys, defaults and refusals. Credentials are not settings here
//! either; the Java reads them from the node's keystore.

use std::path::PathBuf;

/// `serverless.store.type`: `fs` (the default) or `s3`.
pub const TYPE: &str = "serverless.store.type";
/// `serverless.store.path`: the directory of an `fs` store. Required for it.
pub const PATH: &str = "serverless.store.path";
/// `serverless.store.bucket`: the bucket of an `s3` store. Required for it.
pub const BUCKET: &str = "serverless.store.bucket";
/// `serverless.store.region`: the bucket's region, default `us-east-1`.
pub const REGION: &str = "serverless.store.region";
/// `serverless.store.endpoint`: an S3-compatible endpoint; AWS's when absent.
pub const ENDPOINT: &str = "serverless.store.endpoint";
/// `serverless.store.path_style_access`: address the bucket by path, as
/// most S3-compatible services need. Default `false`.
pub const PATH_STYLE_ACCESS: &str = "serverless.store.path_style_access";

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
    /// Reads the store settings through `get`, which answers a setting's
    /// value or `None` when it is unset. The error is the Java's message.
    pub fn parse<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Result<Self, String> {
        // ALLOC: each String below is one setting value or one error message,
        // once per node start; never on a request path.
        let present = |key: &str| get(key).filter(|v| !v.trim().is_empty());
        match get(TYPE).unwrap_or("fs") {
            "fs" => {
                let path = present(PATH)
                    // ALLOC: one setting value or message, once per node start.
                    .ok_or_else(|| format!("{PATH} is required for a filesystem store"))?;
                Ok(StoreSettings::Fs {
                    path: PathBuf::from(path),
                })
            }
            "s3" => {
                let bucket =
                    // ALLOC: one setting value or message, once per node start.
                    present(BUCKET).ok_or_else(|| format!("{BUCKET} is required for an s3 store"))?;
                let path_style_access = match get(PATH_STYLE_ACCESS) {
                    None | Some("false") => false,
                    Some("true") => true,
                    Some(other) => {
                        // ALLOC: one setting value or message, once per node start.
                        return Err(format!(
                            "Failed to parse value [{other}] as only [true] or [false] are allowed."
                        ));
                    }
                };
                Ok(StoreSettings::S3 {
                    // ALLOC: one setting value or message, once per node start.
                    bucket: bucket.to_owned(),
                    // ALLOC: one setting value or message, once per node start.
                    region: get(REGION).unwrap_or("us-east-1").to_owned(),
                    // ALLOC: one setting value or message, once per node start.
                    endpoint: get(ENDPOINT).map(str::to_owned),
                    path_style_access,
                })
            }
            // ALLOC: one setting value or message, once per node start.
            other => Err(format!("unknown {TYPE}: {other}; expected fs or s3")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(pairs: &[(&str, &'static str)]) -> Result<StoreSettings, String> {
        let pairs = pairs.to_vec();
        StoreSettings::parse(|k| pairs.iter().find(|(key, _)| *key == k).map(|(_, v)| *v))
    }

    #[test]
    fn fs_is_the_default_and_needs_a_path() {
        assert_eq!(
            parse(&[(PATH, "/data/store")]),
            Ok(StoreSettings::Fs {
                path: PathBuf::from("/data/store")
            })
        );
        assert_eq!(
            parse(&[(TYPE, "fs"), (PATH, "d")]),
            Ok(StoreSettings::Fs {
                path: PathBuf::from("d")
            })
        );
        let missing = Err("serverless.store.path is required for a filesystem store".to_owned());
        assert_eq!(parse(&[]), missing);
        assert_eq!(parse(&[(TYPE, "fs"), (PATH, "  ")]), missing);
    }

    #[test]
    fn s3_needs_a_bucket_and_defaults_the_rest() {
        assert_eq!(
            parse(&[(TYPE, "s3"), (BUCKET, "b")]),
            Ok(StoreSettings::S3 {
                bucket: "b".to_owned(),
                region: "us-east-1".to_owned(),
                endpoint: None,
                path_style_access: false,
            })
        );
        assert_eq!(
            parse(&[
                (TYPE, "s3"),
                (BUCKET, "b"),
                (REGION, "eu-west-1"),
                (ENDPOINT, "http://127.0.0.1:9000"),
                (PATH_STYLE_ACCESS, "true"),
            ]),
            Ok(StoreSettings::S3 {
                bucket: "b".to_owned(),
                region: "eu-west-1".to_owned(),
                endpoint: Some("http://127.0.0.1:9000".to_owned()),
                path_style_access: true,
            })
        );
        assert_eq!(
            parse(&[(TYPE, "s3")]),
            Err("serverless.store.bucket is required for an s3 store".to_owned())
        );
        assert_eq!(
            parse(&[(TYPE, "s3"), (BUCKET, "b"), (PATH_STYLE_ACCESS, "yes")]),
            Err("Failed to parse value [yes] as only [true] or [false] are allowed.".to_owned())
        );
    }

    #[test]
    fn another_type_is_refused() {
        assert_eq!(
            parse(&[(TYPE, "gcs")]),
            Err("unknown serverless.store.type: gcs; expected fs or s3".to_owned())
        );
    }
}
