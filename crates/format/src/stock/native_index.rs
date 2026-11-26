//! A stock index's metadata as the native restore's inputs: the shard
//! count, the mapping and the settings a Rust index may keep.

use super::index_metadata::StockIndexMetadata;
use crate::json_write::push_string;

/// The settings that name the captured index itself or what a Rust index
/// does not have (replicas); a restore drops them.
const DROPPED: [&str; 11] = [
    "index.uuid",
    "index.history.uuid",
    "index.version.created",
    "index.version.upgraded",
    "index.version.minimum_compatible",
    "index.creation_date",
    "index.provided_name",
    "index.number_of_shards",
    "index.number_of_replicas",
    "index.auto_expand_replicas",
    "index.routing.allocation.include._tier_preference",
];

/// The replication type every 3.x index records; `DOCUMENT` is the default a
/// one-copy index has, so a restore drops it and any other value is refused.
const REPLICATION: (&str, &str) = ("index.replication.type", "DOCUMENT");

/// The setting that names the routing partition size.
const PARTITION_SIZE: &str = "index.routing_partition_size";
/// The setting that holds the shard count.
const SHARDS: &str = "index.number_of_shards";

/// What a stock index becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockNative {
    /// The shard count the stock index had.
    pub number_of_shards: u32,
    /// The mapping, the JSON object, when there was one.
    pub mapping: Option<String>,
    /// The settings kept, a JSON object of flat keys to text.
    pub settings: Vec<u8>,
}

/// Why a stock index cannot become a Rust one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StockRefusal {
    /// The index has no usable shard count.
    NoShardCount,
    /// A routing partition size above 1.
    PartitionSize,
    /// A setting the node neither keeps nor drops.
    Setting(String),
}

impl std::fmt::Display for StockRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoShardCount => write!(f, "the index has no shard count of at least 1"),
            Self::PartitionSize => write!(f, "the index has a routing partition size above 1"),
            Self::Setting(key) => {
                write!(f, "the index sets [{key}], which this node does not keep")
            }
        }
    }
}

impl std::error::Error for StockRefusal {}

impl StockIndexMetadata {
    /// The native form of this index. `keeps` says whether a setting key is
    /// one a Rust index stores; a key that is neither kept nor dropped is
    /// refused, naming it.
    ///
    /// # Errors
    ///
    /// A [`StockRefusal`] for a missing or zero shard count, a partition
    /// size above 1 or a setting the node does not keep.
    pub fn native(&self, keeps: &dyn Fn(&str) -> bool) -> Result<StockNative, StockRefusal> {
        let shards = self
            .setting(SHARDS)
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|n| *n >= 1)
            .ok_or(StockRefusal::NoShardCount)?;
        if let Some(size) = self.setting(PARTITION_SIZE)
            && !size.parse::<u32>().is_ok_and(|n| n <= 1)
        {
            return Err(StockRefusal::PartitionSize);
        }
        // ALLOC: one settings object, bounded by the settings' limit.
        let mut out = vec![b'{'];
        for (key, value) in &self.settings {
            if DROPPED.contains(&key.as_str())
                || key == PARTITION_SIZE
                || (key == REPLICATION.0 && value == REPLICATION.1)
            {
                continue;
            }
            if !keeps(key) {
                // ALLOC: error path; one key.
                return Err(StockRefusal::Setting(key.clone()));
            }
            if out.len() > 1 {
                out.push(b',');
            }
            push_string(&mut out, key);
            out.push(b':');
            push_string(&mut out, value);
        }
        out.push(b'}');
        Ok(StockNative {
            number_of_shards: shards,
            // ALLOC: one mapping, bounded by the blob.
            mapping: self.mapping.clone(),
            settings: out,
        })
    }
}
