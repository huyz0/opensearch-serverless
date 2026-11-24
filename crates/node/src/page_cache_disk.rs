//! The node's page cache, with a disk tier when one is configured.
//!
//! The tier keeps the pages a read fetched, so a later read of them makes no
//! request, and counts reads per page. Its page records live in memory:
//! one for every page its budget could hold, charged to `open_indexes` at
//! start and held for the node's life.

use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_mem::{MemoryBudget, Pool, Reservation};

use crate::assemble::{AssembleError, Config};

/// Bytes a page record costs in memory, rounded up from what the tier
/// reports (`PageCache::stats().disk_metadata_bytes` per page held).
const RECORD_BYTES: u64 = 32;

/// The memory the records of a tier of `bytes` take at `page_size`: one for
/// every page it could hold.
fn record_bytes(bytes: u64, page_size: usize) -> usize {
    let pages = bytes
        .checked_div(u64::try_from(page_size).unwrap_or(u64::MAX))
        .unwrap_or(0);
    usize::try_from(pages.saturating_add(1).saturating_mul(RECORD_BYTES)).unwrap_or(usize::MAX)
}

/// The page cache `config` asks for and the reservation its records hold.
///
/// # Errors
///
/// [`AssembleError::Memory`] when the tier's directory cannot be set up or
/// `open_indexes` cannot hold its records.
pub(crate) fn build(
    config: &Config,
    budget: &MemoryBudget,
) -> Result<(Arc<PageCache>, Option<Reservation>), AssembleError> {
    let mut settings = PageCacheConfig::default();
    let mut records = None;
    if let Some((dir, bytes)) = &config.page_cache_disk {
        let needed = record_bytes(*bytes, settings.page_size);
        records = Some(budget.try_reserve(Pool::OpenIndexes, needed).map_err(|e| {
            AssembleError::Memory(format!(
                "the page cache's disk tier needs {needed} B of records: {e:?}"
            ))
        })?);
        settings.disk_dir = Some(dir.clone());
        settings.disk_budget = *bytes;
    }
    let cache = PageCache::new(settings).map_err(|e| AssembleError::Memory(e.to_string()))?;
    Ok((Arc::new(cache), records))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_mem::BudgetConfig;

    fn budget(open_indexes: usize) -> MemoryBudget {
        MemoryBudget::new(
            &BudgetConfig::new(1 << 30)
                .with_pool(Pool::OpenIndexes, open_indexes)
                .expect("fits"),
        )
    }

    fn config(dir: &std::path::Path, bytes: u64) -> Config {
        let mut c = Config::new("", "n1", [0; 6]);
        c.page_cache_disk = Some((dir.to_path_buf(), bytes));
        c
    }

    // AC-3
    #[test]
    fn the_tiers_records_are_charged_to_open_indexes_and_refused_when_they_do_not_fit() {
        let dir = oss_testkit::TempDir::new("page-cache-disk-records");
        let fits = budget(1 << 20);
        let (cache, held) = build(&config(&dir.path().join("t"), 64 << 20), &fits).expect("built");
        assert!(cache.has_disk());
        assert_eq!(
            fits.in_use(Pool::OpenIndexes),
            record_bytes(64 << 20, 256 << 10)
        );
        drop(held);
        assert_eq!(fits.in_use(Pool::OpenIndexes), 0, "released with the node");
        let tight = budget(4096);
        let refused = build(&config(&dir.path().join("u"), 64 << 20), &tight).err();
        assert!(
            matches!(refused, Some(AssembleError::Memory(_))),
            "{refused:?}"
        );
        let (none, held) = build(&Config::new("", "n1", [0; 6]), &tight).expect("built");
        assert!(!none.has_disk() && held.is_none());
    }

    #[test]
    fn a_record_for_every_page_the_tier_could_hold() {
        assert_eq!(record_bytes(0, 256 << 10), 32);
        assert_eq!(record_bytes(1 << 30, 256 << 10), (4096 + 1) * 32);
        assert_eq!(record_bytes(u64::MAX, 1), usize::MAX);
    }
}
