//! The `OSS_PAGE_CACHE_*` variables: the page cache's disk tier.
//!
//! Without a directory the cache keeps nothing past a read, as it always did.

use std::path::PathBuf;

use super::write_limit::Bounded;

/// The most a tier may be given: 1 PiB, far past any disk and far under what
/// its page records could address.
const MAX_BYTES: u64 = 1 << 50;

/// A disk tier for the page cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskCache {
    /// The directory the tier lives in; emptied when the node starts.
    pub dir: PathBuf,
    /// The most bytes of slots it holds.
    pub bytes: u64,
}

/// Reads the tier: `dir` is `OSS_PAGE_CACHE_DIR` (empty for none); a
/// directory needs `OSS_PAGE_CACHE_BYTES`, a size at least one page.
pub(in crate::boot) fn read(dir: &str, bounded: Bounded<'_>) -> Result<Option<DiskCache>, String> {
    if dir.is_empty() {
        return Ok(None);
    }
    let bytes = bounded("OSS_PAGE_CACHE_BYTES", "0", 0, MAX_BYTES)?;
    if bytes < 1 << 20 {
        return Err(format!(
            "OSS_PAGE_CACHE_DIR is set, so OSS_PAGE_CACHE_BYTES [{bytes}] must give the tier at least 1 MiB"
        ));
    }
    Ok(Some(DiskCache {
        dir: PathBuf::from(dir),
        bytes,
    }))
}
