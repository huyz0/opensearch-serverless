//! Whether a cold open prefetches its files, and the memory it holds while
//! it does.
//!
//! The setting resolves for each open as the index's, else the cluster
//! register's, else the node's, else on. A prefetching open holds the bytes
//! its plan reads from the `open_indexes` pool until the open returns: a
//! pool with no room for them opens the shard without prefetch, as an
//! open that reads file by file does, and closes no other index for it.

use std::collections::BTreeMap;
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_mem::{Pool, Reservation};
use oss_reader::prefetch::{self, OpenPrefetch};
use oss_reader::{IndexMeta, ShardFiles};
use oss_types::settings::{BoolSetting, Layers, Live, Scope, Setting};

use super::SearchService;

/// `serverless.reader.open_prefetch.enabled`: whether a cold open reads the
/// files it reads whole in one round. Default `true`: measured to cut a cold
/// open from six round trips to three with no more requests.
pub const OPEN_PREFETCH_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.reader.open_prefetch.enabled",
    true,
    Scope::Node,
    "On a cold open, fetch the files the engine reads whole in one round.",
);

/// `index.serverless.reader.open_prefetch.enabled`: one index's override of
/// [`OPEN_PREFETCH_ENABLED`].
pub const INDEX_OPEN_PREFETCH_ENABLED: BoolSetting = BoolSetting::new(
    "index.serverless.reader.open_prefetch.enabled",
    true,
    Scope::Index,
    "Whether a cold open of this index fetches the files the engine reads \
     whole in one round, whatever the cluster says.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    OPEN_PREFETCH_ENABLED.setting(),
    INDEX_OPEN_PREFETCH_ENABLED.setting(),
];

/// The layers above an index's own settings.
pub(super) struct Layered {
    cluster: Arc<Live>,
    node: BTreeMap<String, String>,
}

impl Layered {
    /// Layers over the cluster register's live settings and the node's own.
    pub(super) fn new(cluster: Arc<Live>, node: BTreeMap<String, String>) -> Self {
        Self { cluster, node }
    }
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Installs the cluster register's live settings and this node's own,
    /// which decide how an open prefetches. Gives false when installed
    /// already.
    pub fn set_settings(&self, cluster: Arc<Live>, node: BTreeMap<String, String>) -> bool {
        self.layers.set(Layered::new(cluster, node)).is_ok()
    }

    /// Whether an open of an index with `meta` prefetches, by the setting.
    pub fn prefetch_enabled(&self, meta: &IndexMeta) -> bool {
        let none = BTreeMap::new();
        let (cluster, node) = match self.layers.get() {
            Some(layers) => (layers.cluster.current(), &layers.node),
            None => (Arc::default(), &none),
        };
        OPEN_PREFETCH_ENABLED
            .resolve(
                &INDEX_OPEN_PREFETCH_ENABLED,
                &Layers {
                    index: Some(&meta.settings),
                    cluster: cluster.as_ref(),
                    node,
                },
            )
            .map_or(OPEN_PREFETCH_ENABLED.default(), |resolved| resolved.value)
    }

    /// How the open of `files` prefetches, and the `open_indexes` bytes
    /// that hold while it does, held until the guard drops; off, with
    /// nothing held, when the setting is off or the pool has no room.
    pub fn open_prefetch(
        &self,
        files: &ShardFiles,
        meta: &IndexMeta,
    ) -> (OpenPrefetch, Option<Reservation>) {
        if !self.prefetch_enabled(meta) {
            return (OpenPrefetch::off(), None);
        }
        let config = OpenPrefetch::default();
        let page = u64::try_from(self.cache.config().page_size).unwrap_or(u64::MAX);
        let ranges = prefetch::plan(files, &|n| self.cache.is_pinned_name(n), page, &config);
        let bytes = usize::try_from(prefetch::planned_bytes(&ranges)).unwrap_or(usize::MAX);
        match self.budget.try_reserve(Pool::OpenIndexes, bytes) {
            Ok(held) => (config, Some(held)),
            Err(_) => (OpenPrefetch::off(), None),
        }
    }
}
