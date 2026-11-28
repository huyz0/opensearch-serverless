//! How a node is sized and named.

use std::time::Duration;

use oss_format::MAX_POINT_IN_TIME_BYTES;
use oss_janitor::gc;
use oss_meta::DeleteMode;
use oss_writer::bulk::StreamConfig;

/// How a node is sized and named.
#[derive(Clone, Debug)]
pub struct Config {
    /// The deployment's base in the store (empty, or ending in `/`).
    pub base: String,
    /// This node's id.
    pub node_id: String,
    /// The six bytes time-based ids are made with.
    pub address: [u8; 6],
    /// The bytes of `_bulk` bodies in flight, node wide.
    pub bulk_bytes: usize,
    /// The bulks that may hold a share at once.
    pub bulk_holders: usize,
    /// How a `_bulk` body streams.
    pub bulk: StreamConfig,
    /// How long a store call waits for a free I/O buffer before it is refused as busy; zero refuses at once.
    pub store_buffer_wait: Duration,
    /// The buffer a point-in-time request holds.
    pub pit_buffer_bytes: usize,
    /// Whether a create inherits from the matching index template (one more GET).
    pub index_templates: bool,
    /// Whether ingest pipelines are served and run by the writes that name them.
    pub ingest_pipelines: bool,
    /// Whether search pipelines are served and a search may name one.
    pub search_pipelines: bool,
    /// `serverless.ltr.enabled`: serve the learning to rank store and its routes.
    pub ltr: bool,
    /// `serverless.sql.enabled`: serve the SQL endpoint.
    pub sql: bool,
    /// `serverless.ppl.enabled`: serve the PPL endpoint.
    pub ppl: bool,
    /// `serverless.sql.size_limit`: the most rows a statement returns.
    pub sql_size_limit: usize,
    /// `serverless.sql.max_request_bytes`: the largest request body.
    pub sql_max_request_bytes: usize,
    /// `serverless.sql.max_buckets`: the most groups a statement reads.
    pub sql_max_buckets: usize,
    /// `serverless.ism.enabled`: serve the index state management policy routes.
    pub ism: bool,
    /// `serverless.async_search.enabled`: serve the asynchronous search routes.
    pub async_search: bool,
    /// `serverless.ml.enabled`: serve the ML Commons model group routes.
    pub ml: bool,
    /// `serverless.notifications.enabled`: serve the notifications config routes.
    pub notifications: bool,
    /// `serverless.ad.enabled`: serve the anomaly detection detector routes.
    pub ad: bool,
    /// `serverless.observability.enabled`: serve the observability object routes.
    pub observability: bool,
    /// `serverless.reports.enabled`: serve the report definition routes.
    pub reports: bool,
    /// `OSS_PLUGINS`: the plugin policy text; empty names no plugin.
    pub plugin_policy: String,
    /// `serverless.ltr.max_window`: the most hits an `sltr` rescore scores.
    pub ltr_max_window: usize,
    /// `serverless.ltr.cache_ttl`, in seconds.
    pub ltr_cache_ttl_secs: u64,
    /// `serverless.ltr.cache_bytes`: what the model cache may hold.
    pub ltr_cache_bytes: usize,
    /// `OSS_SCRIPT_CACHE_TTL`: seconds a stored script is trusted before the set is checked.
    pub script_cache_ttl_secs: u64,
    /// `serverless.security.enabled`: serve the security configuration API.
    pub security: bool,
    /// The static administrator's name and bcrypt hash, needed with security on.
    pub security_admin: Option<(String, String)>,
    /// The JSON Web Token domain, when a key is set.
    pub security_jwt: Option<crate::security_setting::JwtSettings>,
    /// How long a snapshot of the security configuration is used.
    pub security_refresh_millis: u64,
    /// The bcrypt cost an internal user's password is hashed at.
    pub security_bcrypt_cost: u32,
    /// `serverless.knn.enabled`: whether a create accepts `knn_vector`.
    pub knn: bool,
    /// `serverless.nested.enabled`: whether a create accepts `nested` objects.
    pub nested: bool,
    /// `serverless.geospatial.index_fields`: whether a create accepts `geo_point` fields.
    pub geo: bool,
    /// Whether the read-only snapshot routes are served: `serverless.snapshots.enabled`.
    pub snapshots: bool,
    /// Whether a snapshot can be taken: `serverless.snapshots.take.enabled`, beside `snapshots`.
    pub snapshots_take: bool,
    /// `serverless.snapshots.take.capture_aliases`: a take records each index's aliases.
    pub snapshots_capture_aliases: bool,
    /// `serverless.snapshots.take.partial_shards`: a `partial` take records the shards that published nothing.
    pub snapshots_partial_shards: bool,
    /// `serverless.snapshots.take.capture_global_state`: a take with `include_global_state: true` records the global state.
    pub snapshots_capture_global_state: bool,
    /// Whether a shallow snapshot can be restored: `serverless.snapshots.restore.enabled`.
    pub snapshots_restore: bool,
    /// `serverless.snapshots.restore.background.*`: a restore may copy on a thread of its own.
    pub restore_background: crate::restore_background::Settings,
    /// `serverless.snapshots.stock.enabled`: restore from a stock OpenSearch repository, beside `snapshots_restore`.
    pub snapshots_stock: bool,
    /// `OSS_STOCK_REPO_ROOTS` and `OSS_STOCK_REPO_S3_ROOTS`: where a stock repository may sit.
    pub stock_roots: (String, String),
    /// `OSS_STOCK_RESTORE_FANOUT`: how many metadata blobs a stock restore reads at once.
    pub stock_fanout: usize,
    /// `serverless.snapshots.stock.export.enabled`: export a snapshot into a stock repository.
    pub snapshots_stock_export: bool,
    /// `OSS_STOCK_EXPORT_FS_ROOTS` and `OSS_STOCK_EXPORT_S3_ROOTS`: where an export may write.
    pub stock_export_roots: (String, String),
    /// The endpoint a stock `s3` repository is read through; none when this node's store is a directory.
    pub stock_s3: Option<crate::stock_s3::SourceS3>,
    /// `serverless.system_indices.patterns`: comma-separated `*` patterns of the names a take and a restore refuse.
    pub system_index_patterns: String,
    /// `serverless.plugins.upstreams`: Java nodes serving unported plugins; none proxies nothing.
    pub plugin_upstreams: Vec<String>,
    /// `serverless.plugins.max_request_bytes`: the most a proxied body may be.
    pub plugin_max_request_bytes: u64,
    /// `serverless.plugins.timeout`: the wait for a proxied request's connect and answer.
    pub plugin_timeout: Duration,
    /// How a point in time's register is deleted.
    pub delete_mode: DeleteMode,
    /// How often the publish timer runs a pass.
    pub publish_every: Duration,
    /// `serverless.wal.node_log.publish_max_age_millis`.
    pub publish_age_millis: u64,
    /// `serverless.publish.max_unpublished_operations`.
    pub publish_max_operations: u64,
    /// The write log's window, which a due time rounds up to.
    pub publish_tick_millis: u64,
    /// How long a writer shard may go unused before it hibernates; 0 never.
    pub hibernate_after_millis: u64,
    /// How long a hibernated shard may sleep before its head is given back; 0 never.
    pub give_back_after_millis: u64,
    /// The most writer shards this node holds; 0 no cap.
    pub max_shards: usize,
    /// How long a shard must be unused before a node at its cap lets it go; 0 never.
    pub evict_after_millis: u64,
    /// Whether idle shards are handed to a member with room.
    pub hand_off: bool,
    /// Checkpoint age of staged commits; none writes each register.
    pub commit_checkpoint_millis: Option<u64>,
    /// How small segment blobs are packed; none writes a blob each.
    pub node_packs: Option<crate::writer::PackConfig>,
    /// Whether staged commits ride the node log; needs staging.
    pub commit_logged: crate::boot::Logged,
    /// Merging a shard's small segments.
    pub merge: crate::writer::MergeConfig,
    /// How far below the cap making room clears, as a percent of it.
    pub evict_headroom_percent: u64,
    /// Indexes whose shards are never let go of.
    pub pinned_indexes: Vec<String>,
    /// What limits the writes waiting on the log.
    pub write_limit: oss_writer::admission::LimiterConfig,
    /// How often the janitor runs a pass.
    pub janitor_every: Duration,
    /// How a janitor sweep behaves; it starts as a dry run.
    pub sweep: gc::Config,
    /// The most shards an index may have to be purged inside its delete;
    /// a larger one is purged at the reclaim.
    pub inline_purge_max_shards: u32,
    /// How often this node drains the reclaim queue.
    pub reclaim_every: Duration,
    /// The most points in time that may be live for a janitor pass to run.
    pub janitor_max_views: usize,
    /// A disk tier for the page cache: its directory and the most bytes of
    /// slots it holds; none keeps nothing past a read.
    pub page_cache_disk: Option<(std::path::PathBuf, u64)>,
    /// The most bytes of demand-filled pages kept resident; 0 leaves the
    /// engine's default.
    pub resident_page_bytes: u64,
}

impl Config {
    /// The defaults for node `node_id` under `base`.
    pub fn new(base: &str, node_id: &str, address: [u8; 6]) -> Self {
        Self {
            // ALLOC: two names, once a node.
            base: base.to_owned(),
            node_id: node_id.to_owned(),
            address,
            bulk_bytes: 64 << 20,
            bulk_holders: 64,
            bulk: StreamConfig::default(),
            store_buffer_wait: Duration::from_secs(5),
            pit_buffer_bytes: MAX_POINT_IN_TIME_BYTES,
            index_templates: false,
            ingest_pipelines: false,
            search_pipelines: false,
            ltr: false,
            sql: false,
            ppl: false,
            sql_size_limit: 200,
            sql_max_request_bytes: 1 << 20,
            sql_max_buckets: 10_000,
            ism: false,
            async_search: true,
            ml: false,
            notifications: true,
            ad: false,
            observability: false,
            reports: false,
            plugin_policy: String::new(),
            ltr_max_window: 1_000,
            ltr_cache_ttl_secs: 30,
            ltr_cache_bytes: 64 << 20,
            script_cache_ttl_secs: 10,
            security: false,
            security_admin: None,
            security_jwt: None,
            security_refresh_millis: 10_000,
            security_bcrypt_cost: crate::security_route::DEFAULT_COST,
            knn: false,
            nested: false,
            geo: false,
            snapshots: false,
            snapshots_take: false,
            snapshots_capture_aliases: false,
            snapshots_partial_shards: false,
            snapshots_capture_global_state: false,
            snapshots_restore: false,
            restore_background: crate::restore_background::Settings::off(),
            snapshots_stock: false,
            stock_roots: (String::new(), String::new()),
            stock_fanout: 8,
            snapshots_stock_export: false,
            stock_export_roots: (String::new(), String::new()),
            stock_s3: None,
            system_index_patterns: String::new(),
            plugin_upstreams: Vec::new(),
            plugin_max_request_bytes: 10 << 20,
            plugin_timeout: Duration::from_secs(30),
            delete_mode: DeleteMode::Tombstone,
            publish_every: Duration::from_millis(250),
            publish_age_millis: 1_000,
            publish_max_operations: 10_000,
            publish_tick_millis: 250,
            hibernate_after_millis: 0,
            give_back_after_millis: 0,
            max_shards: 0,
            evict_after_millis: 60_000,
            hand_off: false,
            commit_checkpoint_millis: None,
            commit_logged: crate::boot::Logged::Off,
            node_packs: None,
            merge: crate::writer::MergeConfig::off(),
            evict_headroom_percent: 0,
            pinned_indexes: Vec::new(),
            write_limit: oss_writer::admission::LimiterConfig::default(),
            janitor_every: Duration::from_secs(60),
            sweep: gc::Config {
                dry_run: false,
                minimum_unreferenced_millis: 60_000,
                max_watched: 100_000,
                max_pinned: 100_000,
                relist_after_millis: 3_600_000,
            },
            inline_purge_max_shards: 16,
            reclaim_every: Duration::from_secs(60),
            janitor_max_views: 10_000,
            page_cache_disk: None,
            resident_page_bytes: 0,
        }
    }
}
