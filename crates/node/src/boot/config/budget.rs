//! The node's memory split into pools.

use oss_mem::{BudgetConfig, ConfigError as BudgetError, Pool};

use super::NodeConfig;

/// The shares of the page cache's 20% that other pools take; 0 reserves none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Percents {
    /// `OSS_EXTENSIONS_PERCENT`: the `extensions` pool.
    pub extensions: usize,
    /// `OSS_ML_MODELS_PERCENT`: the `models` pool for local ML models.
    pub models: usize,
}

/// Reads each share in its range.
pub(super) fn read_percents(
    bounded: &impl Fn(&str, &str, u64, u64) -> Result<u64, String>,
) -> Result<Percents, String> {
    let get = |name: &str| -> Result<usize, String> {
        Ok(usize::try_from(bounded(name, "0", 0, 20)?).unwrap_or(0))
    };
    Ok(Percents {
        extensions: get("OSS_EXTENSIONS_PERCENT")?,
        models: get("OSS_ML_MODELS_PERCENT")?,
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
            self.pool_percents.extensions
        } else {
            0
        };
        // Both borrow from the page cache's 20%, so together they take at most that.
        let models = self.pool_percents.models.min(20_usize.saturating_sub(ext));
        BudgetConfig::new(self.memory_bytes)
            .with_pool(Pool::RequestBodies, share(20))?
            .with_pool(Pool::QueryExecution, share(15))?
            .with_pool(Pool::WriteBuffers, share(15))?
            .with_pool(
                Pool::PageCache,
                share(20_usize.saturating_sub(ext).saturating_sub(models)),
            )?
            .with_pool(Pool::Extensions, share(ext))?
            .with_pool(Pool::Models, share(models))?
            .with_pool(Pool::Transport, share(10))?
            .with_pool(Pool::OpenIndexes, share(20))
    }
}
