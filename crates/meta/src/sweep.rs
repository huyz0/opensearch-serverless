//! Removing tombstones once their quarantine has passed.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/TombstoneSweep.java`.
//! A tombstone guards a swap in flight from before its delete; no swap is in
//! flight for hours, and past that a tombstone is a register every listing
//! reads. Each delete filed a marker under its UTC hour
//! (`tombstones/<hour>/<name>`), so a sweep finds the names to remove from
//! the markers of hours old enough, never by walking every index.
//!
//! Removal is a claim then a conditional delete at the claimed generation,
//! so a sweep running late, or two sweeps of one hour, cannot remove a name
//! created again: only a store whose conditional delete was proved by
//! [`crate::DeleteMode::probe`] is swept at all.

use oss_blob::{MAX_LIST_KEYS, SwapOutcome};
use oss_format::{MAX_TOMBSTONE_BYTES, TOMBSTONE_PREFIX, Tombstone, tombstone_bucket_start};

use crate::descriptors::{
    BUCKET_MILLIS, DeleteMode, DescriptorStore, REAP_DEADLINE_MILLIS, REAP_HANDOFF_MILLIS,
};
use crate::error::MetaError;

/// Where under a deployment's base markers are filed (`RegisterMap.tombstones`).
pub use oss_format::MARKERS_PREFIX;

/// What one sweep is to do.
pub struct Sweep<'a> {
    /// The current time, in the clock the tombstones were stamped with.
    pub now_millis: i64,
    /// How long a tombstone must have stood.
    pub quarantine_millis: i64,
    /// Markers listed a page at a time, at most [`MAX_LIST_KEYS`].
    pub page: usize,
    /// Whether this caller sweeps a bucket: a hint drawn from membership,
    /// since two sweeps of one bucket contend on the claim and one loses.
    pub owns_bucket: &'a dyn Fn(&str) -> bool,
    /// The clock a claim is stamped with and its deadline checked against.
    pub clock: &'a dyn Fn() -> i64,
}

/// What a sweep did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Swept {
    /// Tombstones removed.
    pub removed: u64,
    /// Markers whose name was live, gone, or deleted again later: dropped.
    pub stale: u64,
    /// Markers kept for a later sweep: inside their quarantine, another
    /// sweep's claim, or a claim that missed its deadline.
    pub kept: u64,
}

/// What became of one marker.
enum Reaped {
    Removed,
    Stale,
    Kept,
}

impl DescriptorStore {
    /// Removes the tombstones whose quarantine has passed, from the buckets
    /// `sweep.owns_bucket` accepts, calling `removed` with each name. Only
    /// in [`DeleteMode::Conditional`]: otherwise a late delete could land on
    /// a name created again, so tombstones stay and nothing is sent.
    ///
    /// Markers are walked in key order, a page at a time. Bucket names are
    /// fixed-width hours and sort by time, so the walk stops at the first
    /// bucket too young to hold an expired name and never lists its markers.
    ///
    /// # Errors
    ///
    /// [`MetaError::Blob`] when the store fails; what was done before
    /// stands, and a later sweep picks up the rest.
    pub fn sweep_tombstones(
        &self,
        sweep: &Sweep<'_>,
        buf: &mut [u8],
        removed: &mut dyn FnMut(&str),
    ) -> Result<Swept, MetaError> {
        let mut swept = Swept::default();
        if self.deletes != DeleteMode::Conditional {
            return Ok(swept);
        }
        // ALLOC: one prefix a sweep.
        let prefix = [self.base.as_str(), MARKERS_PREFIX].concat();
        let page = sweep.page.clamp(1, MAX_LIST_KEYS);
        // ALLOC: one page of keys, at most `page` keys of MAX_KEY_BYTES each.
        let mut keys: Vec<String> = Vec::with_capacity(page);
        let mut after: Option<String> = None;
        loop {
            keys.clear();
            // The markers of expired hours, a page at a time; the walk stops
            // at the first young hour, so it scales with deletes, not indexes.
            let listed = self
                .store
                // LIST: markers of expired hours only, as above.
                .list_prefix(&prefix, after.as_deref(), page, &mut |key, _| {
                    // ALLOC: one key, bounded as above.
                    keys.push(key.to_owned());
                })?;
            let mut next = None;
            let mut skipping: Option<&str> = None;
            for key in &keys {
                let marker = key.get(prefix.len()..).unwrap_or_default();
                let Some((bucket, name)) = marker.split_once('/') else {
                    continue;
                };
                if skipping == Some(bucket) || name.contains('/') {
                    continue;
                }
                skipping = None;
                let Some(start) = tombstone_bucket_start(bucket) else {
                    skipping = Some(bucket);
                    continue;
                };
                if start.saturating_add(sweep.quarantine_millis) > sweep.now_millis {
                    // This hour and every later one are too young.
                    return Ok(swept);
                }
                if !(sweep.owns_bucket)(bucket) {
                    skipping = Some(bucket);
                    continue;
                }
                match self.reap(name, start, sweep, buf)? {
                    Reaped::Kept => {
                        swept.kept = swept.kept.saturating_add(1);
                        continue;
                    }
                    Reaped::Stale => swept.stale = swept.stale.saturating_add(1),
                    Reaped::Removed => {
                        swept.removed = swept.removed.saturating_add(1);
                        removed(name);
                    }
                }
                self.store.delete(key)?;
            }
            // A store claiming more with an empty page would start the walk
            // over: stop instead.
            if !listed.truncated || keys.is_empty() {
                return Ok(swept);
            }
            if let Some(bucket) = skipping {
                // The rest of a skipped bucket: list past it, not through it.
                // ALLOC: one cursor a page.
                next = Some([prefix.as_str(), bucket, "0"].concat());
            }
            after = next.or_else(|| keys.last().cloned());
        }
    }

    fn reap(
        &self,
        name: &str,
        bucket_start: i64,
        sweep: &Sweep<'_>,
        buf: &mut [u8],
    ) -> Result<Reaped, MetaError> {
        // A marker this store did not file cannot be a register key.
        let Ok(key) = self.existing_key(name) else {
            return Ok(Reaped::Kept);
        };
        let Some(read) = self.store.read_register(&key, buf)? else {
            return Ok(Reaped::Stale);
        };
        let value = buf.get(..read.len).unwrap_or_default();
        if !value.starts_with(TOMBSTONE_PREFIX) {
            // Created again since, or a delete whose swap lost after its
            // marker was filed.
            return Ok(Reaped::Stale);
        }
        // A malformed tombstone is left for a person, not removed; the Java
        // fails the whole sweep on it.
        let Ok(tombstone) = Tombstone::parse(value) else {
            return Ok(Reaped::Kept);
        };
        if tombstone.deleted_at >= bucket_start.saturating_add(BUCKET_MILLIS) {
            // Created and deleted again later, which filed its own marker.
            return Ok(Reaped::Stale);
        }
        if sweep.now_millis.saturating_sub(tombstone.deleted_at) < sweep.quarantine_millis {
            // Stamped by a clock ahead of this one: not this sweep's to shorten.
            return Ok(Reaped::Kept);
        }
        let claim_at = (sweep.clock)();
        if tombstone.reaping_at != 0
            && claim_at < tombstone.reaping_at.saturating_add(REAP_HANDOFF_MILLIS)
        {
            // Another sweep's claim, not yet abandoned.
            return Ok(Reaped::Kept);
        }
        let mut claim = [0u8; MAX_TOMBSTONE_BYTES];
        let len = Tombstone {
            deleted_at: tombstone.deleted_at,
            reaping_at: claim_at,
        }
        .write(&mut claim)?;
        let SwapOutcome::Applied(claimed) = self.store.swap_register(
            &key,
            read.generation,
            claim.get(..len).unwrap_or_default(),
        )?
        else {
            return Ok(Reaped::Kept);
        };
        if (sweep.clock)() >= claim_at.saturating_add(REAP_DEADLINE_MILLIS) {
            // Too late to be sure no create took the claim over. Left
            // claimed; a later sweep claims it afresh after the handoff.
            return Ok(Reaped::Kept);
        }
        Ok(if self.store.delete_register(&key, claimed)? {
            Reaped::Removed
        } else {
            Reaped::Kept
        })
    }
}
