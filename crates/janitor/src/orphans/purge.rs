//! Deleting one orphan shard container: its blobs in batches, the `manifest`
//! register last, so an interrupted deletion is judged again by the next
//! cycle.

use oss_blob::{BlobStore, MAX_DELETE_KEYS, MAX_LIST_KEYS};
use oss_format::MANIFEST;

use crate::JanitorError;

/// Lists the container under `prefix` (ending in `/`) and, unless `dry_run`,
/// deletes what it lists, the register last; returns how many blobs it held.
/// A blob already gone counts as deleted: another slice's work.
///
/// Cost: `max(1, ceil(b / 1000))` LISTs, `ceil(b / 1000)` DELETE batches and
/// one DELETE of the register for `b` blobs.
pub(super) fn purge(
    store: &dyn BlobStore,
    prefix: &str,
    dry_run: bool,
) -> Result<u64, JanitorError> {
    let register = format!("{prefix}{MANIFEST}");
    let mut counted = 0u64;
    let mut register_seen = false;
    let mut after: Option<String> = None;
    loop {
        // ALLOC: one key a blob of a page, at most MAX_LIST_KEYS.
        let mut batch: Vec<String> = Vec::new();
        let mut last: Option<String> = None;
        // LIST: one page of the orphan's blobs, 1,000 keys.
        let page = store.list_prefix(prefix, after.as_deref(), MAX_LIST_KEYS, &mut |key, _| {
            if key == register {
                register_seen = true;
            } else {
                // ALLOC: one key, as above.
                batch.push(key.to_owned());
            }
            // ALLOC: the page's last key.
            last = Some(key.to_owned());
        })?;
        counted = counted.saturating_add(u64::try_from(batch.len()).unwrap_or(u64::MAX));
        if !dry_run {
            for chunk in batch.chunks(MAX_DELETE_KEYS) {
                // ALLOC: one reference a key of the batch.
                let keys: Vec<&str> = chunk.iter().map(String::as_str).collect();
                store.delete_many(&keys)?;
            }
        }
        if !page.truncated || last.is_none() {
            break;
        }
        after = last;
    }
    if register_seen {
        counted = counted.saturating_add(1);
        if !dry_run {
            store.delete(&register)?;
        }
    }
    Ok(counted)
}
