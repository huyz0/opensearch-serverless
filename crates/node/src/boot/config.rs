//! A node's configuration, read from named variables.
//!
//! The lookup is a function, so a test supplies variables without touching
//! the process environment.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use oss_janitor::settings::{ORPHAN_SLICE_BUDGET, ORPHAN_SLICE_INTERVAL_MILLIS};
use oss_mem::{BudgetConfig, ConfigError as BudgetError, Pool};

use crate::tail_overlay::REFRESH_VISIBILITY;

pub(super) mod apply;
pub(super) mod page_cache;
pub(super) mod roles;
pub(super) mod search_slots;
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
    /// `serverless.janitor.dry_run`: count what the janitor would delete and
    /// delete nothing. Off by default.
    pub janitor_dry_run: bool,
    /// `serverless.buckets.enabled`: own and publish a shard whose index
    /// descriptor names a bucket through the bucket. Off by default.
    pub buckets_enabled: bool,
    /// `serverless.snapshots.enabled`: serve the read-only snapshot routes. Off by default.
    pub snapshots_enabled: bool,
    /// `serverless.snapshots.take.enabled`: take shallow snapshots, beside the snapshot routes. Off by default.
    pub snapshots_take_enabled: bool,
    /// `serverless.snapshots.restore.enabled`: restore shallow snapshots, beside the take. Off by default.
    pub snapshots_restore_enabled: bool,
    /// `serverless.plugins.upstreams`: `http(s)` bases of Java nodes that serve the plugins not ported; none by default.
    pub plugin_upstreams: Vec<String>,
    /// `serverless.plugins.max_request_bytes`.
    pub plugin_max_request_bytes: u64,
    /// `serverless.plugins.timeout`, in milliseconds.
    pub plugin_timeout_millis: u64,
    /// `serverless.search.drain.settle`, in milliseconds: how long a drain waits for the lease to lose the search role; two thirds of the lease TTL unless set.
    pub drain_settle_millis: u64,
    /// `serverless.activation.hibernate_after_millis`; 0 never hibernates.
    pub hibernate_after_millis: u64,
    /// `serverless.search.commit_push.enabled`: a published commit is pushed
    /// to the other readers of its shard. On by default, off by default when the
    /// tail overlay is on (the overlay serves freshness, and the push made its
    /// readers reopen cold).
    pub commit_push: bool,
    /// `serverless.activation.give_back_after_millis`; 0 never gives back.
    pub give_back_after_millis: u64,
    /// `OSS_RETIRE_TIMEOUT_MS`: how long a node told to stop spends
    /// publishing its shards and giving their heads back; 0 skips it and
    /// leaves the shards to the lease.
    pub retire_timeout_millis: u64,
    /// `serverless.activation.max_shards`; 0 is no cap.
    pub max_shards: usize,
    /// `serverless.activation.evict_after_millis`; 0 never evicts.
    pub evict_after_millis: u64,
    /// `serverless.activation.evict_headroom_percent`.
    pub evict_headroom_percent: u64,
    /// `serverless.activation.pinned_indexes`, split at commas.
    pub pinned_indexes: Vec<String>,
    /// `serverless.activation.hand_off.enabled`.
    pub hand_off: bool,
    /// `serverless.security.enabled`: serve the security configuration API. Off by default.
    pub security: bool,
    /// The static administrator's name and bcrypt hash (`OSS_SECURITY_ADMIN_USER`, `OSS_SECURITY_ADMIN_HASH`).
    pub security_admin: Option<(String, String)>,
    /// `OSS_SECURITY_REFRESH_MS`: how long a snapshot of the security configuration is used.
    pub security_refresh_millis: u64,
    /// `serverless.knn.enabled`.
    pub knn: bool,
    /// `OSS_STORE_USAGE`: count the store's requests and bytes per index
    /// and serve them at `GET /_serverless/usage`.
    pub store_usage: bool,
    /// `OSS_STORE_USAGE_MAX_INDICES`: how many indices the usage tracks
    /// before counting the rest together.
    pub store_usage_max_indices: usize,
    /// `OSS_TENANT_LEVELS`: `off`, `observe` or `enforce`; with store usage
    /// on, each tenant's cost is judged against its budget and the level is
    /// reported at `GET /_serverless/usage`. Only `enforce` slows a tenant.
    pub tenant_levels: &'static str,
    /// `OSS_TENANT_BUDGET_PER_MINUTE`: the cost units a minute a tenant may
    /// use before its level rises (the large tier).
    pub tenant_budget_per_minute: u64,
    /// `OSS_TENANT_BUDGET_SMALL`: the small tier's budget.
    pub tenant_budget_small: u64,
    /// `OSS_TENANT_BUDGET_STANDARD`: the standard tier's budget.
    pub tenant_budget_standard: u64,
    /// `OSS_TENANT_DEFAULT_TIER`: the tier of an index that sets none,
    /// `small`, `standard`, `large` or empty for the mode's (large in
    /// observe, standard in enforce).
    pub tenant_default_tier: &'static str,
    /// `OSS_TENANT_TICK_MS`: how often tenant levels are stepped.
    pub tenant_tick_millis: u64,
    /// `serverless.write.backpressure.*`: what limits the writes waiting on
    /// the write log.
    pub write_limit: oss_writer::admission::LimiterConfig,
    /// `OSS_SEARCH_*`: the gate that orders searches under pressure;
    /// `slots` 0 is off.
    pub search_slots: crate::search_slots::Config,
    /// `OSS_PAGE_CACHE_DIR` and `OSS_PAGE_CACHE_BYTES`: a disk tier for the
    /// page cache; none keeps nothing past a read.
    pub page_cache: Option<page_cache::DiskCache>,
    /// `OSS_ROLES`: the roles the node's lease names, `ingest` and `search`;
    /// none is a coordinating-only node.
    pub roles: Vec<&'static str>,
    /// How often the one sweeping node removes expired tombstones; 0 is off.
    pub tombstone_sweep_millis: u64,
    /// `serverless.janitor.orphan_slice_budget`: the shard containers one
    /// orphan-sweep slice judges; 0, the default, is off.
    pub orphan_slice_budget: usize,
    /// `serverless.janitor.orphan_slice_interval_millis`.
    pub orphan_slice_millis: u64,
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
        let janitor_dry_run = flag("OSS_JANITOR_DRY_RUN", "false")?;
        let buckets_enabled = flag("OSS_BUCKETS_ENABLED", "false")?;
        let snapshots_enabled = flag("OSS_SNAPSHOTS_ENABLED", "false")?;
        let snapshots_take_enabled = flag("OSS_SNAPSHOTS_TAKE_ENABLED", "false")?;
        let snapshots_restore_enabled = flag("OSS_SNAPSHOTS_RESTORE_ENABLED", "false")?;
        let plugin_upstreams = or("OSS_PLUGIN_UPSTREAMS", "")
            .split(',')
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let plugin_max_request_bytes =
            bounded("OSS_PLUGIN_MAX_REQUEST_BYTES", "10485760", 1, 1 << 40)?;
        let plugin_timeout_millis = bounded("OSS_PLUGIN_TIMEOUT_MS", "30000", 1, 3_600_000)?;
        let drain_default = (lease_ttl_millis.saturating_mul(2) / 3).clamp(1, 60_000);
        let drain_settle_millis =
            bounded("OSS_DRAIN_SETTLE_MS", &drain_default.to_string(), 1, 60_000)?;
        let tail_overlay = flag("OSS_TAIL_OVERLAY", "true")?;
        let commit_push = flag(
            "OSS_COMMIT_PUSH",
            if tail_overlay { "false" } else { "true" },
        )?;
        let proactive_takeover = flag("OSS_PROACTIVE_TAKEOVER", "true")?;
        let hibernate_after_millis = bounded("OSS_HIBERNATE_AFTER_MS", "0", 0, 604_800_000)?;
        let give_back_after_millis = bounded("OSS_GIVE_BACK_AFTER_MS", "0", 0, 86_400_000)?;
        let retire_timeout_millis = bounded("OSS_RETIRE_TIMEOUT_MS", "25000", 0, 3_600_000)?;
        let max_shards = bounded("OSS_MAX_SHARDS", "0", 0, 1_000_000)?;
        let evict_after_millis = bounded("OSS_EVICT_AFTER_MS", "60000", 0, 604_800_000)?;
        let evict_headroom_percent = bounded("OSS_EVICT_HEADROOM_PCT", "0", 0, 100)?;
        let pinned_indexes = or("OSS_PINNED_INDEXES", "")
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect();
        let hand_off = flag("OSS_HAND_OFF", "false")?;
        let knn = flag("OSS_KNN", "false")?;
        let security = flag("OSS_SECURITY", "false")?;
        let security_refresh_millis = bounded("OSS_SECURITY_REFRESH_MS", "10000", 1000, 3_600_000)?;
        let security_admin = match (
            get("OSS_SECURITY_ADMIN_USER").filter(|v| !v.is_empty()),
            get("OSS_SECURITY_ADMIN_HASH").filter(|v| !v.is_empty()),
        ) {
            (Some(user), Some(hash)) => Some((user, hash)),
            (None, None) => None,
            _ => {
                return Err(
                    "OSS_SECURITY_ADMIN_USER and OSS_SECURITY_ADMIN_HASH go together".to_owned(),
                );
            }
        };
        let store_usage = flag("OSS_STORE_USAGE", "false")?;
        let store_usage_max_indices = bounded("OSS_STORE_USAGE_MAX_INDICES", "1000", 1, 1_000_000)?;
        let tenant_levels = match or("OSS_TENANT_LEVELS", "observe").as_str() {
            "off" => "off",
            "observe" => "observe",
            "enforce" => "enforce",
            other => {
                return Err(format!(
                    "OSS_TENANT_LEVELS is [{other}], not off, observe or enforce"
                ));
            }
        };
        let tenant_budget_per_minute =
            bounded("OSS_TENANT_BUDGET_PER_MINUTE", "2000", 1, 100_000_000)?;
        let tenant_budget_small = bounded("OSS_TENANT_BUDGET_SMALL", "20", 1, 100_000_000)?;
        let tenant_budget_standard = bounded("OSS_TENANT_BUDGET_STANDARD", "200", 1, 100_000_000)?;
        let tenant_default_tier = match or("OSS_TENANT_DEFAULT_TIER", "").as_str() {
            "" => "",
            "small" => "small",
            "standard" => "standard",
            "large" => "large",
            other => {
                return Err(format!(
                    "OSS_TENANT_DEFAULT_TIER is [{other}], not small, standard or large"
                ));
            }
        };
        let tenant_tick_millis = bounded("OSS_TENANT_TICK_MS", "10000", 10, 3_600_000)?;
        let write_limit = write_limit::read(&bounded)?;
        let search_slots = search_slots::read(&bounded)?;
        let page_cache = page_cache::read(&or("OSS_PAGE_CACHE_DIR", ""), &bounded)?;
        let roles = roles::read(get("OSS_ROLES"))?;
        let proactive_reapers = bounded("OSS_PROACTIVE_REAPERS", "2", 1, 16)?;
        let proactive_max_concurrent = bounded("OSS_PROACTIVE_MAX_CONCURRENT", "4", 1, 64)?;
        let tombstone_sweep_millis = bounded("OSS_TOMBSTONE_SWEEP_MS", "3600000", 0, 604_800_000)?;
        let orphan_slice_budget = ORPHAN_SLICE_BUDGET
            .parse(&or(
                "OSS_ORPHAN_SLICE_BUDGET",
                &ORPHAN_SLICE_BUDGET.default().to_string(),
            ))
            .map_err(|e| format!("OSS_ORPHAN_SLICE_BUDGET: {e}"))?;
        let orphan_slice_millis = ORPHAN_SLICE_INTERVAL_MILLIS
            .parse(&or(
                "OSS_ORPHAN_SLICE_MS",
                &ORPHAN_SLICE_INTERVAL_MILLIS.default().to_string(),
            ))
            .map_err(|e| format!("OSS_ORPHAN_SLICE_MS: {e}"))?;
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
            search_slots,
            page_cache,
            roles,
            orphan_slice_budget: usize::try_from(orphan_slice_budget).unwrap_or(0),
            orphan_slice_millis: u64::try_from(orphan_slice_millis).unwrap_or(u64::MAX),
            janitor_dry_run,
            buckets_enabled,
            snapshots_enabled,
            snapshots_take_enabled,
            snapshots_restore_enabled,
            plugin_upstreams,
            plugin_max_request_bytes,
            plugin_timeout_millis,
            drain_settle_millis,
            hibernate_after_millis,
            commit_push,
            give_back_after_millis,
            retire_timeout_millis,
            max_shards: usize::try_from(max_shards).unwrap_or(0),
            evict_after_millis,
            evict_headroom_percent,
            pinned_indexes,
            hand_off,
            security,
            security_admin,
            security_refresh_millis,
            knn,
            store_usage,
            tenant_levels,
            tenant_budget_per_minute,
            tenant_budget_small,
            tenant_budget_standard,
            tenant_default_tier,
            tenant_tick_millis,
            store_usage_max_indices: usize::try_from(store_usage_max_indices).unwrap_or(1000),
            proactive_takeover,
            proactive_reapers: usize::try_from(proactive_reapers).unwrap_or(2),
            proactive_max_concurrent: usize::try_from(proactive_max_concurrent).unwrap_or(4),
            proactive_pass_millis,
            tail_overlay,
            refresh_visibility,
            cluster_settings_poll_millis,
        })
    }

    /// Whether the node takes writes: its lease names `ingest`.
    pub fn takes_writes(&self) -> bool {
        self.roles.contains(&"ingest")
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
mod security_tests;
#[cfg(test)]
mod tests;
