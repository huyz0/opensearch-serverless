//! The node's memory split into pools.

use oss_mem::{BudgetConfig, ConfigError as BudgetError, Pool};

use super::NodeConfig;

impl NodeConfig {
    /// The node's memory split into pools by fixed shares.
    ///
    /// # Errors
    ///
    /// [`BudgetError`] when the shares do not fit the total.
    pub fn budget(&self) -> Result<BudgetConfig, BudgetError> {
        let share = |percent: usize| (self.memory_bytes / 100).saturating_mul(percent);
        let ext = if cfg!(feature = "extensions") {
            self.extensions_percent
        } else {
            0
        };
        BudgetConfig::new(self.memory_bytes)
            .with_pool(Pool::RequestBodies, share(20))?
            .with_pool(Pool::QueryExecution, share(15))?
            .with_pool(Pool::WriteBuffers, share(15))?
            .with_pool(Pool::PageCache, share(20_usize.saturating_sub(ext)))?
            .with_pool(Pool::Extensions, share(ext))?
            .with_pool(Pool::Transport, share(10))?
            .with_pool(Pool::OpenIndexes, share(20))
    }
}
