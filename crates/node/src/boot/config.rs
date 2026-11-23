//! A node's configuration, read from named variables.
//!
//! The lookup is a function, so a test supplies variables without touching
//! the process environment.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use oss_mem::{BudgetConfig, ConfigError as BudgetError, Pool};

use crate::tail_overlay::REFRESH_VISIBILITY;

pub(super) mod write_limit;

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
    /// Where the transport server listens (port 0 for any free port).
    pub transport: SocketAddr,
    /// The transport address this node's lease carries; the address the
    /// server bound when unset.
    pub advertise: Option<String>,
    /// The node's memory, split into pools.
    pub memory_bytes: usize,
    /// How far ahead a lease renewal stamps its expiry.
    pub lease_ttl_millis: i64,
    /// How often the node reads the fleet's leases again; unset, the same
    /// as the lease is renewed (a third of its lifetime).
    pub lease_refresh_millis: Option<u64>,
    /// How long a writer trusts an index it read before confirming it by
    /// the change stripes; zero reads the descriptor on every lookup.
    pub index_confirm_millis: u64,
    /// How often the janitor sweeps and the dead-log pass runs; it is also
    /// the first wait before a dead node's log is tried again.
    pub janitor_every_millis: u64,
    /// `serverless.activation.hibernate_after_millis`; 0 never hibernates.
    pub hibernate_after_millis: u64,
    /// `serverless.write.backpressure.*`: what limits the writes waiting on
    /// the write log.
    pub write_limit: oss_writer::admission::LimiterConfig,
    /// How often the one sweeping node removes expired tombstones; 0 is off.
    pub tombstone_sweep_millis: u64,
    /// `serverless.takeover.proactive.enabled`: whether this node takes the
    /// shards a dead node's log names before a write asks.
    pub proactive_takeover: bool,
    /// `serverless.takeover.proactive.reapers`.
    pub proactive_reapers: usize,
    /// `serverless.takeover.proactive.max_concurrent`.
    pub proactive_max_concurrent: usize,
    /// `serverless.takeover.proactive.pass_millis`.
    pub proactive_pass_millis: u64,
    /// `serverless.search.tail_overlay.enabled`: whether writers push their
    /// acknowledged writes to readers and searches see them.
    pub tail_overlay: bool,
    /// `serverless.search.refresh_visibility`: `owner`, `serving` or
    /// `everywhere`.
    pub refresh_visibility: &'static str,
    /// `OSS_CLUSTER_SETTINGS_POLL_MS`: how often a node with the tail overlay
    /// on reads the cluster settings register, in milliseconds.
    pub cluster_settings_poll_millis: u64,
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
        let lease_refresh_millis = match get("OSS_LEASE_REFRESH_MS").filter(|v| !v.is_empty()) {
            None => None,
            Some(v) => {
                let millis: u64 = v
                    .parse()
                    .map_err(|e| format!("OSS_LEASE_REFRESH_MS: {e}"))?;
                if millis == 0 {
                    return Err("OSS_LEASE_REFRESH_MS is below 1".to_owned());
                }
                Some(millis)
            }
        };
        let index_confirm_millis: u64 = or("OSS_INDEX_CONFIRM_MS", "5000")
            .parse()
            .map_err(|e| format!("OSS_INDEX_CONFIRM_MS: {e}"))?;
        let bounded = |key: &str, default: &str, low: u64, high: u64| -> Result<u64, String> {
            let value: u64 = or(key, default)
                .parse()
                .map_err(|e| format!("{key}: {e}"))?;
            if (low..=high).contains(&value) {
                Ok(value)
            } else {
                Err(format!("{key} [{value}] is not within {low} to {high}"))
            }
        };
        let flag = |key: &str, default: &str| match or(key, default).as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(format!("{key} is [{other}], not true or false")),
        };
        let janitor_every_millis = bounded("OSS_JANITOR_EVERY_MS", "60000", 1, u64::MAX)?;
        let tail_overlay = flag("OSS_TAIL_OVERLAY", "false")?;
        let proactive_takeover = flag("OSS_PROACTIVE_TAKEOVER", "true")?;
        let hibernate_after_millis = bounded("OSS_HIBERNATE_AFTER_MS", "0", 0, 604_800_000)?;
        let write_limit = write_limit::read(&bounded)?;
        let proactive_reapers = bounded("OSS_PROACTIVE_REAPERS", "2", 1, 16)?;
        let proactive_max_concurrent = bounded("OSS_PROACTIVE_MAX_CONCURRENT", "4", 1, 64)?;
        let tombstone_sweep_millis = bounded("OSS_TOMBSTONE_SWEEP_MS", "3600000", 0, 604_800_000)?;
        let proactive_pass_millis = bounded("OSS_PROACTIVE_PASS_MS", "30000", 1, 3_600_000)?;
        let cluster_settings_poll_millis =
            bounded("OSS_CLUSTER_SETTINGS_POLL_MS", "30000", 1, 3_600_000)?;
        let refresh_visibility = REFRESH_VISIBILITY
            .parse(&or("OSS_REFRESH_VISIBILITY", REFRESH_VISIBILITY.default()))
            .map_err(|e| format!("OSS_REFRESH_VISIBILITY: {e}"))?;
        Ok(Self {
            node_id,
            cluster: or("OSS_CLUSTER", "opensearch-serverless"),
            base,
            store,
            http,
            transport: or("OSS_TRANSPORT_ADDR", "127.0.0.1:9300")
                .parse()
                .map_err(|e| format!("OSS_TRANSPORT_ADDR: {e}"))?,
            advertise: get("OSS_ADVERTISE_ADDR").filter(|v| !v.is_empty()),
            memory_bytes,
            lease_ttl_millis,
            lease_refresh_millis,
            index_confirm_millis,
            janitor_every_millis,
            tombstone_sweep_millis,
            write_limit,
            hibernate_after_millis,
            proactive_takeover,
            proactive_reapers: usize::try_from(proactive_reapers).unwrap_or(2),
            proactive_max_concurrent: usize::try_from(proactive_max_concurrent).unwrap_or(4),
            proactive_pass_millis,
            tail_overlay,
            refresh_visibility,
            cluster_settings_poll_millis,
        })
    }

    /// How often the lease is renewed: a third of its lifetime.
    pub fn renew_every(&self) -> Duration {
        Duration::from_millis(u64::try_from(self.lease_ttl_millis / 3).unwrap_or(1).max(1))
    }

    /// How often the fleet's leases are read again: `OSS_LEASE_REFRESH_MS`,
    /// or as often as the lease is renewed.
    pub fn refresh_every(&self) -> Duration {
        self.lease_refresh_millis
            .map_or_else(|| self.renew_every(), Duration::from_millis)
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
    fn the_lease_refresh_interval_is_set_apart_from_the_renewal() {
        let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_LEASE_REFRESH_MS", "2000")])))
            .expect("valid");
        assert_eq!(c.refresh_every(), Duration::from_secs(2));
        assert_eq!(
            c.renew_every(),
            Duration::from_secs(10),
            "the renewal stays"
        );
    }

    #[test]
    fn a_file_store_node_takes_the_defaults() {
        let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
        assert_eq!(c.node_id, "n1");
        assert_eq!(c.cluster, "opensearch-serverless");
        assert_eq!(c.base, "");
        assert_eq!(c.http, "127.0.0.1:9200".parse().expect("addr"));
        assert_eq!(c.transport, "127.0.0.1:9300".parse().expect("addr"));
        assert_eq!(c.advertise, None);
        assert_eq!(c.memory_bytes, 1 << 30);
        assert_eq!(c.lease_ttl_millis, 30_000);
        assert_eq!(c.lease_refresh_millis, None);
        assert_eq!(c.cluster_settings_poll_millis, 30_000);
        assert_eq!(c.refresh_every(), c.renew_every(), "as often as renewed");
        assert_eq!(c.index_confirm_millis, 5_000);
        assert_eq!(c.janitor_every_millis, 60_000);
        assert_eq!(c.renew_every(), Duration::from_secs(10));
        assert_eq!(
            c.store,
            StoreChoice::Fs {
                root: "/data".into()
            }
        );
    }

    #[test]
    fn proactive_takeover_is_on_with_the_settings_defaults_and_each_is_read() {
        let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
        assert!(c.proactive_takeover);
        let off = NodeConfig::from_lookup(lookup(&with(&[("OSS_PROACTIVE_TAKEOVER", "false")])))
            .expect("valid");
        assert!(!off.proactive_takeover);
        assert_eq!(
            (
                c.proactive_reapers,
                c.proactive_max_concurrent,
                c.proactive_pass_millis
            ),
            (2, 4, 30_000)
        );
        let on = NodeConfig::from_lookup(lookup(&with(&[
            ("OSS_PROACTIVE_TAKEOVER", "true"),
            ("OSS_PROACTIVE_REAPERS", "3"),
            ("OSS_PROACTIVE_MAX_CONCURRENT", "8"),
            ("OSS_PROACTIVE_PASS_MS", "5000"),
        ])))
        .expect("valid");
        assert!(on.proactive_takeover);
        assert_eq!(
            (
                on.proactive_reapers,
                on.proactive_max_concurrent,
                on.proactive_pass_millis
            ),
            (3, 8, 5_000)
        );
    }

    #[test]
    fn a_proactive_setting_out_of_its_bounds_is_refused() {
        for (key, bad) in [
            ("OSS_PROACTIVE_TAKEOVER", "yes"),
            ("OSS_PROACTIVE_REAPERS", "0"),
            ("OSS_PROACTIVE_REAPERS", "17"),
            ("OSS_PROACTIVE_MAX_CONCURRENT", "0"),
            ("OSS_PROACTIVE_MAX_CONCURRENT", "65"),
            ("OSS_PROACTIVE_PASS_MS", "0"),
            ("OSS_PROACTIVE_PASS_MS", "3600001"),
            ("OSS_PROACTIVE_PASS_MS", "x"),
        ] {
            let err = NodeConfig::from_lookup(lookup(&with(&[(key, bad)]))).expect_err(key);
            assert!(err.contains(key), "{err}");
        }
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
        refused(
            &with(&[("OSS_TRANSPORT_ADDR", "nope")]),
            "OSS_TRANSPORT_ADDR",
        );
        refused(&with(&[("OSS_MEMORY_BYTES", "x")]), "OSS_MEMORY_BYTES");
        refused(&with(&[("OSS_MEMORY_BYTES", "1000")]), "OSS_MEMORY_BYTES");
        refused(&with(&[("OSS_LEASE_TTL_MS", "y")]), "OSS_LEASE_TTL_MS");
        refused(
            &with(&[("OSS_LEASE_REFRESH_MS", "y")]),
            "OSS_LEASE_REFRESH_MS",
        );
        refused(
            &with(&[("OSS_LEASE_REFRESH_MS", "0")]),
            "OSS_LEASE_REFRESH_MS",
        );
        refused(
            &with(&[("OSS_CLUSTER_SETTINGS_POLL_MS", "0")]),
            "OSS_CLUSTER_SETTINGS_POLL_MS",
        );
        refused(
            &with(&[("OSS_CLUSTER_SETTINGS_POLL_MS", "soon")]),
            "OSS_CLUSTER_SETTINGS_POLL_MS",
        );
        refused(&with(&[("OSS_LEASE_TTL_MS", "2")]), "OSS_LEASE_TTL_MS");
        refused(
            &with(&[("OSS_JANITOR_EVERY_MS", "0")]),
            "OSS_JANITOR_EVERY_MS",
        );
        refused(
            &with(&[("OSS_JANITOR_EVERY_MS", "x")]),
            "OSS_JANITOR_EVERY_MS",
        );
        refused(&with(&[("OSS_TAIL_OVERLAY", "yes")]), "OSS_TAIL_OVERLAY");
        refused(
            &with(&[("OSS_REFRESH_VISIBILITY", "all")]),
            "OSS_REFRESH_VISIBILITY",
        );
    }

    #[test]
    fn the_tail_overlay_is_off_and_visibility_everywhere_unless_set() {
        let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
        assert!(!c.tail_overlay);
        assert_eq!(c.refresh_visibility, "everywhere");
        let mut pairs = FS.to_vec();
        pairs.push(("OSS_TAIL_OVERLAY", "true"));
        pairs.push(("OSS_REFRESH_VISIBILITY", "serving"));
        let c = NodeConfig::from_lookup(lookup(&pairs)).expect("valid");
        assert!(c.tail_overlay);
        assert_eq!(c.refresh_visibility, "serving");
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
