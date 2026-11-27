//! A node's use of packs: the settings, and the one pack writer a node makes
//! when its first publish asks for it.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use oss_blob::BlobStore;
use oss_types::settings::{BoolSetting, LongSetting, Scope, Setting};
use oss_writer::pack::{PackIndexStore, PackLedger, PackStore, PackWriter};
use oss_writer::publish::PackPlacement;

/// The most bytes a pack grows to before it is written without waiting out
/// the linger.
const MAX_PACK_BYTES: u64 = 64 << 20;

/// `serverless.segments.node_pack.enabled`: small segment blobs go into
/// packs shared by the node's shards. Default `false`: no collector exists
/// yet, so a pack is never deleted.
pub const ENABLED: BoolSetting = BoolSetting::new(
    "serverless.segments.node_pack.enabled",
    false,
    Scope::Node,
    "A publish's small segment blobs join one pack shared by every shard the node publishes \
     within the linger: one PUT instead of one per segment. Every lone publish waits the linger.",
);

/// `serverless.segments.node_pack.max_blob_bytes`: the largest blob packed.
pub const MAX_BLOB_BYTES: LongSetting = LongSetting::new(
    "serverless.segments.node_pack.max_blob_bytes",
    8 << 20,
    1,
    64 << 20,
    Scope::Node,
    "With node packs on, the largest segment blob that goes into a pack; larger ones keep a \
     blob of their own.",
);

/// `serverless.segments.node_pack.linger_millis`: how long a pack waits for
/// other shards' blobs.
pub const LINGER_MILLIS: LongSetting = LongSetting::new(
    "serverless.segments.node_pack.linger_millis",
    50,
    1,
    5_000,
    Scope::Node,
    "With node packs on, how long the first blob of a pack waits for other shards' blobs, in \
     milliseconds.",
);

/// `serverless.segments.node_pack.publish_threads`: how many shards a publish
/// pass publishes at once, so their blobs share a pack.
pub const PUBLISH_THREADS: LongSetting = LongSetting::new(
    "serverless.segments.node_pack.publish_threads",
    8,
    1,
    64,
    Scope::Node,
    "With node packs on, the most due shards one publish pass publishes at once, so their small \
     blobs share a pack; 1 publishes them one after another, each in a pack of its own.",
);

/// `serverless.segments.node_pack.collect`: the writer reserves the ledger's
/// `high` ahead and rotates its epoch, so a collector can track every pack.
pub const COLLECT: BoolSetting = BoolSetting::new(
    "serverless.segments.node_pack.collect",
    false,
    Scope::Node,
    "With node packs on, the writer reserves its ledger's high ahead in blocks and starts a new \
     epoch every 1,024 packs, so a collector can name every pack without listing; about one \
     more small PUT in eight packs. Does nothing while packs are off.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    ENABLED.setting(),
    PUBLISH_THREADS.setting(),
    COLLECT.setting(),
    MAX_BLOB_BYTES.setting(),
    LINGER_MILLIS.setting(),
];

/// How a node packs: the largest blob packed and how long a pack lingers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackConfig {
    /// The largest segment blob that goes into a pack.
    pub max_blob_bytes: u64,
    /// How long the first blob of a pack waits for others.
    pub linger: Duration,
    /// The most due shards a publish pass publishes at once.
    pub publish_threads: usize,
    /// Reserve the ledger ahead and rotate epochs, for a collector.
    pub collect: bool,
}

impl Default for PackConfig {
    fn default() -> Self {
        Self {
            max_blob_bytes: 8 << 20,
            linger: Duration::from_millis(50),
            publish_threads: 8,
            collect: false,
        }
    }
}

/// The node's pack writer, made on first use.
pub(super) struct Packs {
    config: PackConfig,
    writer: Mutex<Option<Arc<PackWriter>>>,
}

impl Packs {
    /// How many shards a pass publishes at once.
    pub(super) fn threads(&self) -> usize {
        self.config.publish_threads
    }

    pub(super) fn new(config: PackConfig) -> Self {
        Self {
            config,
            writer: Mutex::new(None),
        }
    }

    /// Where `shard` of `index` (`uuid`) puts its small blobs: the node's
    /// writer, made now if this is the first.
    ///
    /// # Errors
    ///
    /// The ledger's failure to name an epoch; the next call tries again.
    pub(super) fn placement(
        &self,
        store: &Arc<dyn BlobStore>,
        base: &str,
        node: &str,
        clock_millis: u64,
        shard: (&str, &str, u32),
    ) -> Result<PackPlacement, String> {
        let mut held = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let writer = if let Some(writer) = held.as_ref() {
            Arc::clone(writer)
        } else {
            let ledger =
                PackLedger::new(Arc::clone(store), base, node).map_err(|e| e.to_string())?;
            let epoch = ledger.next_epoch(clock_millis).map_err(|e| e.to_string())?;
            let mut writer = PackWriter::new(
                PackStore::new(Arc::clone(store), base, node),
                Some(ledger),
                epoch,
                self.config.linger,
                MAX_PACK_BYTES,
            )
            .with_collection(self.config.collect);
            if self.config.collect {
                writer = writer.with_index(PackIndexStore::new(Arc::clone(store), base), node);
            }
            let made = Arc::new(writer);
            *held = Some(Arc::clone(&made));
            made
        };
        Ok(PackPlacement {
            writer,
            // ALLOC: two names a publish.
            index_name: shard.0.to_owned(),
            index_uuid: shard.1.to_owned(),
            shard: shard.2,
            max_blob_bytes: self.config.max_blob_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // AC-1
    #[test]
    fn the_settings_default_off_with_the_documented_sizes_and_bounds() {
        assert!(!ENABLED.default());
        assert_eq!(MAX_BLOB_BYTES.default(), 8 << 20);
        assert_eq!(LINGER_MILLIS.default(), 50);
        assert_eq!(PackConfig::default().max_blob_bytes, 8 << 20);
        assert_eq!(PackConfig::default().linger, Duration::from_millis(50));
        let bounds = |s: &Setting| format!("{s:?}");
        assert!(bounds(MAX_BLOB_BYTES.setting()).contains("67108864"));
        assert!(bounds(LINGER_MILLIS.setting()).contains("5000"));
    }
}
