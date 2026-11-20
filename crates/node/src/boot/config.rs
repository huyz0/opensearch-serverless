//! A node's configuration, read from named variables.
//!
//! The lookup is a function, so a test supplies variables without touching
//! the process environment.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use oss_mem::{BudgetConfig, ConfigError as BudgetError, Pool};

/// The smallest memory total the pools' shares make workable.
pub const MIN_MEMORY_BYTES: usize = 512 << 20;

/// Where a node keeps its blobs.
#[derive(Clone, PartialEq, Eq)]
pub enum StoreChoice {
    /// A directory on this host.
    Fs {
        /// The directory.
        root: PathBuf,
    },
    /// An S3 bucket.
    S3 {
        /// `http://host:port` or `https://host`.
        endpoint: String,
        /// The bucket.
        bucket: String,
        /// The signing region.
        region: String,
        /// The access key id.
        access_key: String,
        /// The secret; never printed.
        secret_key: String,
    },
}

impl fmt::Debug for StoreChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fs { root } => f.debug_struct("Fs").field("root", root).finish(),
            Self::S3 {
                endpoint,
                bucket,
                region,
                access_key,
                ..
            } => f
                .debug_struct("S3")
                .field("endpoint", endpoint)
                .field("bucket", bucket)
                .field("region", region)
                .field("access_key", access_key)
                .field("secret_key", &"<redacted>")
                .finish(),
        }
    }
}

/// How a node is configured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeConfig {
    /// This node's stable id.
    pub node_id: String,
    /// The cluster's name, as health reports it.
    pub cluster: String,
    /// The deployment's base in the store: empty, or ending in `/`.
    pub base: String,
    /// Where blobs live.
    pub store: StoreChoice,
    /// Where the HTTP server listens.
    pub http: SocketAddr,
    /// The transport address this node's lease carries.
    pub advertise: String,
    /// The node's memory, split into pools.
    pub memory_bytes: usize,
    /// How far ahead a lease renewal stamps its expiry.
    pub lease_ttl_millis: i64,
}

impl NodeConfig {
    /// The configuration `get` names; a variable it does not know is unset.
    ///
    /// # Errors
    ///
    /// A message naming the variable that is missing or unusable.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let required = |key: &str| {
            get(key)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{key} is required"))
        };
        let or = |key: &str, default: &str| {
            get(key)
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| default.to_owned())
        };
        let node_id = required("OSS_NODE_ID")?;
        let store = match required("OSS_STORE")?.as_str() {
            "fs" => StoreChoice::Fs {
                root: PathBuf::from(required("OSS_FS_ROOT")?),
            },
            "s3" => StoreChoice::S3 {
                endpoint: required("OSS_S3_ENDPOINT")?,
                bucket: required("OSS_S3_BUCKET")?,
                region: required("OSS_S3_REGION")?,
                access_key: required("OSS_S3_ACCESS_KEY")?,
                secret_key: required("OSS_S3_SECRET_KEY")?,
            },
            other => return Err(format!("OSS_STORE is [{other}], not fs or s3")),
        };
        let base = or("OSS_BASE", "");
        if !base.is_empty() && !base.ends_with('/') {
            return Err(format!("OSS_BASE [{base}] must be empty or end in /"));
        }
        let http = or("OSS_HTTP_ADDR", "127.0.0.1:9200")
            .parse()
            .map_err(|e| format!("OSS_HTTP_ADDR: {e}"))?;
        let memory_bytes: usize = or("OSS_MEMORY_BYTES", &(1usize << 30).to_string())
            .parse()
            .map_err(|e| format!("OSS_MEMORY_BYTES: {e}"))?;
        if memory_bytes < MIN_MEMORY_BYTES {
            return Err(format!(
                "OSS_MEMORY_BYTES [{memory_bytes}] is below the {MIN_MEMORY_BYTES} the pools need"
            ));
        }
        let lease_ttl_millis: i64 = or("OSS_LEASE_TTL_MS", "30000")
            .parse()
            .map_err(|e| format!("OSS_LEASE_TTL_MS: {e}"))?;
        if lease_ttl_millis < 3 {
            return Err(format!("OSS_LEASE_TTL_MS [{lease_ttl_millis}] is below 3"));
        }
        Ok(Self {
            node_id,
            cluster: or("OSS_CLUSTER", "opensearch-serverless"),
            base,
            store,
            http,
            advertise: or("OSS_ADVERTISE_ADDR", "127.0.0.1:9300"),
            memory_bytes,
            lease_ttl_millis,
        })
    }

    /// How often the lease is renewed: a third of its lifetime.
    pub fn renew_every(&self) -> Duration {
        Duration::from_millis(u64::try_from(self.lease_ttl_millis / 3).unwrap_or(1).max(1))
    }

    /// The node's memory split into pools by fixed shares.
    ///
    /// # Errors
    ///
    /// [`BudgetError`] when the shares do not fit the total.
    pub fn budget(&self) -> Result<BudgetConfig, BudgetError> {
        let share = |percent: usize| (self.memory_bytes / 100).saturating_mul(percent);
        BudgetConfig::new(self.memory_bytes)
            .with_pool(Pool::RequestBodies, share(20))?
            .with_pool(Pool::QueryExecution, share(15))?
            .with_pool(Pool::WriteBuffers, share(15))?
            .with_pool(Pool::PageCache, share(20))?
            .with_pool(Pool::Transport, share(10))?
            .with_pool(Pool::OpenIndexes, share(20))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    const FS: &[(&str, &str)] = &[
        ("OSS_NODE_ID", "n1"),
        ("OSS_STORE", "fs"),
        ("OSS_FS_ROOT", "/data"),
    ];

    fn with<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut all: Vec<(&str, &str)> = FS.to_vec();
        all.extend_from_slice(extra);
        all
    }

    #[test]
    fn a_file_store_node_takes_the_defaults() {
        let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
        assert_eq!(c.node_id, "n1");
        assert_eq!(c.cluster, "opensearch-serverless");
        assert_eq!(c.base, "");
        assert_eq!(c.http, "127.0.0.1:9200".parse().expect("addr"));
        assert_eq!(c.advertise, "127.0.0.1:9300");
        assert_eq!(c.memory_bytes, 1 << 30);
        assert_eq!(c.lease_ttl_millis, 30_000);
        assert_eq!(c.renew_every(), Duration::from_secs(10));
        assert_eq!(
            c.store,
            StoreChoice::Fs {
                root: "/data".into()
            }
        );
    }

    #[test]
    fn an_s3_node_names_its_bucket() {
        let pairs = [
            ("OSS_NODE_ID", "n1"),
            ("OSS_STORE", "s3"),
            ("OSS_S3_ENDPOINT", "http://127.0.0.1:9000"),
            ("OSS_S3_BUCKET", "b"),
            ("OSS_S3_REGION", "r"),
            ("OSS_S3_ACCESS_KEY", "ak"),
            ("OSS_S3_SECRET_KEY", "sk"),
            ("OSS_BASE", "prod/"),
        ];
        let c = NodeConfig::from_lookup(lookup(&pairs)).expect("valid");
        assert_eq!(c.base, "prod/");
        assert!(matches!(c.store, StoreChoice::S3 { ref bucket, .. } if bucket == "b"));
    }

    #[test]
    fn what_cannot_be_used_is_refused_by_name() {
        let refused = |pairs: &[(&str, &str)], needle: &str| {
            let e = NodeConfig::from_lookup(lookup(pairs)).expect_err("refused");
            assert!(e.contains(needle), "{e} lacks {needle}");
        };
        refused(&[], "OSS_NODE_ID");
        refused(&[("OSS_NODE_ID", "n")], "OSS_STORE");
        refused(&[("OSS_NODE_ID", "n"), ("OSS_STORE", "gcs")], "[gcs]");
        refused(&[("OSS_NODE_ID", "n"), ("OSS_STORE", "fs")], "OSS_FS_ROOT");
        refused(
            &[("OSS_NODE_ID", "n"), ("OSS_STORE", "s3")],
            "OSS_S3_ENDPOINT",
        );
        refused(&with(&[("OSS_BASE", "prod")]), "OSS_BASE");
        refused(&with(&[("OSS_HTTP_ADDR", "nope")]), "OSS_HTTP_ADDR");
        refused(&with(&[("OSS_MEMORY_BYTES", "x")]), "OSS_MEMORY_BYTES");
        refused(&with(&[("OSS_MEMORY_BYTES", "1000")]), "OSS_MEMORY_BYTES");
        refused(&with(&[("OSS_LEASE_TTL_MS", "y")]), "OSS_LEASE_TTL_MS");
        refused(&with(&[("OSS_LEASE_TTL_MS", "2")]), "OSS_LEASE_TTL_MS");
    }

    #[test]
    fn the_secret_is_not_printed() {
        let pairs = [
            ("OSS_NODE_ID", "n1"),
            ("OSS_STORE", "s3"),
            ("OSS_S3_ENDPOINT", "http://e"),
            ("OSS_S3_BUCKET", "b"),
            ("OSS_S3_REGION", "r"),
            ("OSS_S3_ACCESS_KEY", "ak"),
            ("OSS_S3_SECRET_KEY", "hunter2"),
        ];
        let c = NodeConfig::from_lookup(lookup(&pairs)).expect("valid");
        let shown = format!("{c:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("ak"));
    }

    #[test]
    fn the_pools_fit_and_every_pool_is_given_bytes() {
        let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
        let b = c.budget().expect("fits");
        let sum: usize = Pool::ALL.iter().map(|p| b.pool(*p)).sum();
        assert!(sum <= c.memory_bytes);
        assert!(Pool::ALL.iter().all(|p| b.pool(*p) > 0));
        assert_eq!(b.pool(Pool::OpenIndexes), c.memory_bytes / 100 * 20);
    }
}
