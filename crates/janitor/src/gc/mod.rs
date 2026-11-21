//! The garbage collector: segment blobs no commit names.
//!
//! Ported from the shell's `reconcile/GarbageCollector.java` (`sweepShard`).
//! [`Collector::sweep`] judges one shard. The rule, the Java's whole safety
//! argument: a blob is deleted only when it is in a term container strictly
//! older than the published manifest's term (publishing is not atomic, so
//! the live term holds files not yet named) **and** nothing names it, not
//! the manifest (a failover inherits files, so a live commit routinely names
//! older terms' blobs) and not a pin. A blob must also have been seen so by
//! the sweep before: a reader serves the commit it opened until its next
//! backstop, and what that commit no longer names is exactly what is
//! collectable.
//! A listing here is one shard's, priced like a PUT.

mod memory;
mod pins;
mod point_in_time;
mod terms;

pub use memory::Memory;
pub use pins::{Pinned, Pins};
pub use point_in_time::PointInTimePins;

use std::collections::{BTreeMap, HashSet};

use oss_blob::{BlobStore, MAX_LIST_KEYS};
use oss_format::{CommitManifest, ShardRef};

use crate::JanitorError;

/// How a sweep behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Count what would be deleted and delete nothing
    /// ([`crate::settings::DRY_RUN`]).
    pub dry_run: bool,
    /// How long, in the clock's milliseconds, a blob must have been seen
    /// unreferenced before it may go, on top of the two sightings. Zero is a
    /// pure pass count.
    pub minimum_unreferenced_millis: i64,
    /// The most unreferenced blobs a sweep watches or deletes at once;
    /// blobs past it are judged by later sweeps. Bounds the memory.
    pub max_watched: usize,
    /// The most blob keys the pins may name for one shard; more is as if
    /// they said [`Pinned::Everything`], so nothing is deleted.
    pub max_pinned: usize,
}

/// What kind of sweep this was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The shard was judged.
    Swept,
    /// The shard has no manifest: a writer may be mid-first-publish, and
    /// nothing was listed.
    NoCommit,
    /// The pins could not say what they hold: nothing was deleted, and the
    /// memory is as it was.
    Pinned,
}

/// What one sweep of one shard did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sweep {
    /// How the sweep went.
    pub outcome: Outcome,
    /// The keys deleted.
    pub deleted: Vec<String>,
    /// The keys a dry run would have deleted.
    pub would_delete: Vec<String>,
    /// The blobs now being watched: unreferenced, not yet deletable.
    pub watching: usize,
}

/// Whether a blob first seen at `since` is old enough to delete at `now`.
fn aged_since(since: Option<i64>, now: i64, config: &Config) -> bool {
    since.is_some_and(|since| now.saturating_sub(since) >= config.minimum_unreferenced_millis)
}

impl Sweep {
    fn empty(outcome: Outcome) -> Self {
        Self {
            outcome,
            deleted: Vec::new(),
            would_delete: Vec::new(),
            watching: 0,
        }
    }
}

/// Sweeps shards of one deployment.
pub struct Collector<'a> {
    store: &'a dyn BlobStore,
    base: &'a str,
    pins: &'a dyn Pins,
    clock: &'a dyn Fn() -> i64,
    config: Config,
}

impl<'a> Collector<'a> {
    /// A collector over `store` under `base`, reading the time from `clock`.
    pub fn new(
        store: &'a dyn BlobStore,
        base: &'a str,
        pins: &'a dyn Pins,
        clock: &'a dyn Fn() -> i64,
        config: Config,
    ) -> Self {
        Self {
            store,
            base,
            pins,
            clock,
            config,
        }
    }

    /// Sweeps `shard`: reads its manifest into `buf` (a pooled buffer big
    /// enough for one), asks the pins, lists its storage path and deletes
    /// what the rule allows.
    ///
    /// # Errors
    ///
    /// [`JanitorError`] for a store failure, a manifest that is malformed or
    /// does not fit `buf`, or pins that cannot be read. Nothing is deleted
    /// before the listing, and a failed delete leaves `memory` as it was.
    pub fn sweep(
        &self,
        shard: ShardRef<'_>,
        memory: &mut Memory,
        buf: &mut [u8],
    ) -> Result<Sweep, JanitorError> {
        let path = shard.base_path(self.base)?;
        let now = (self.clock)();
        // The manifest first, then the pins, then the listing: a pin made
        // while this sweep runs reads this manifest or a later one, so its
        // files cannot be orphans of the commit this sweep observed.
        let Some(read) = self
            .store
            .read_register(&shard.manifest_key(self.base)?, buf)?
        else {
            return Ok(Sweep::empty(Outcome::NoCommit));
        };
        let manifest = CommitManifest::parse(buf.get(..read.len).unwrap_or_default())?;
        let live_term = manifest.term;
        // ALLOC: one key a file the manifest names, at most the manifest's file limit.
        let mut referenced: HashSet<String> = HashSet::new();
        for extent in shard.resolve(self.base, &manifest)? {
            referenced.insert(extent.key);
        }
        match self.pins.pinned(&shard)? {
            Pinned::Everything => return Ok(Sweep::empty(Outcome::Pinned)),
            // ALLOC: one key a blob a pin names, bounded by the shard's blobs.
            Pinned::Blobs(keys) if keys.len() > self.config.max_pinned => {
                return Ok(Sweep::empty(Outcome::Pinned));
            }
            Pinned::Blobs(keys) => referenced.extend(keys),
        }

        let empty = BTreeMap::new();
        let before = memory.seen(&path).unwrap_or(&empty);
        // ALLOC: one entry a blob seen unreferenced and not yet deletable,
        // bounded by the shard's unreferenced blobs.
        let mut watching: BTreeMap<String, i64> = BTreeMap::new();
        // ALLOC: one key a blob to delete this sweep, same bound.
        let mut due: Vec<String> = Vec::new();
        let mut start_after: Option<String> = None;
        loop {
            let mut last: Option<String> = None;
            // LIST: one shard's storage path, a page of 1,000 keys at a time;
            // the cost scales with the shard's blobs and nothing else.
            let page = self.store.list_prefix(
                &path,
                start_after.as_deref(),
                MAX_LIST_KEYS,
                &mut |key, _| {
                    last = Some(key.to_owned());
                    if !terms::is_older_term_blob(key, &path, live_term) || referenced.contains(key)
                    {
                        return;
                    }
                    let since = before.get(key).copied();
                    let aged = aged_since(since, now, &self.config);
                    // The cap holds for every blob but one already due, so a
                    // lowered cap shrinks the memory and an aged blob is never starved.
                    if watching.len() >= self.config.max_watched && !aged {
                        return;
                    }
                    if aged {
                        due.push(key.to_owned());
                    }
                    watching.insert(key.to_owned(), since.unwrap_or(now));
                },
            )?;
            if !page.truncated {
                break;
            }
            start_after = last;
            if start_after.is_none() {
                break;
            }
        }

        let mut sweep = Sweep::empty(Outcome::Swept);
        if self.config.dry_run {
            // What a dry run would delete stays watched, so the sweep after
            // the switch flips deletes it at once.
            sweep.would_delete = due;
        } else {
            let keys: Vec<&str> = due.iter().map(String::as_str).collect();
            self.store.delete_many(&keys)?;
            for key in &due {
                watching.remove(key);
            }
            sweep.deleted = due;
        }
        sweep.watching = watching.len();
        memory.set(&path, watching);
        Ok(sweep)
    }
}
