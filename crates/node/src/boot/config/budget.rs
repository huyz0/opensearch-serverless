//! The node's memory split into pools.

use oss_mem::{BudgetConfig, ConfigError as BudgetError, Pool};

use super::NodeConfig;

/// What the `extensions` and `training` pools take from the page cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolSizes {
    /// `OSS_EXTENSIONS_PERCENT`: the page cache's share the `extensions` pool takes; 0 reserves none.
    pub extensions_percent: usize,
    /// `OSS_KNN_TRAINING_MB`: the bytes the `training` pool may hold, at most a quarter of the page cache; 0 reserves none.
    pub knn_training_bytes: usize,
}

/// Reads each size in its range.
pub(super) fn read(
    bounded: &impl Fn(&str, &str, u64, u64) -> Result<u64, String>,
) -> Result<PoolSizes, String> {
    let to_usize = |v: u64| usize::try_from(v).unwrap_or(0);
    Ok(PoolSizes {
        extensions_percent: to_usize(bounded("OSS_EXTENSIONS_PERCENT", "0", 0, 20)?),
        knn_training_bytes: to_usize(bounded("OSS_KNN_TRAINING_MB", "256", 0, 65_536)?)
            .saturating_mul(1 << 20),
    })
}

impl NodeConfig {
    /// The node's memory split into pools by fixed shares.
    ///
    /// # Errors
    ///
    /// [`BudgetError`] when the shares do not fit the total.
    pub fn budget(&self) -> Result<BudgetConfig, BudgetError> {
        let share = |percent: usize| (self.memory_bytes / 100).saturating_mul(percent);
        let ext = if cfg!(feature = "extensions") {
            self.pool_sizes.extensions_percent
        } else {
            0
        };
        // The page cache, less what extensions take; training takes at most a quarter of that.
        let cache = share(20_usize.saturating_sub(ext));
        let training = if self.knn {
            self.pool_sizes.knn_training_bytes.min(cache / 4)
        } else {
            0
        };
        BudgetConfig::new(self.memory_bytes)
            .with_pool(Pool::RequestBodies, share(20))?
            .with_pool(Pool::QueryExecution, share(15))?
            .with_pool(Pool::WriteBuffers, share(15))?
            .with_pool(Pool::PageCache, cache.saturating_sub(training))?
            .with_pool(Pool::Training, training)?
            .with_pool(Pool::Extensions, share(ext))?
            .with_pool(Pool::Transport, share(10))?
            .with_pool(Pool::OpenIndexes, share(20))
    }
}
