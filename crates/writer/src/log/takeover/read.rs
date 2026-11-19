//! A shard's records in a fenced log, read blob by blob from each epoch's
//! last header: one ranged read a blob for its header, kept for the other
//! shards in the same blob, and one a slice of the shard's, or one for a
//! whole body shared by several shards. Nothing lists the store.
//!
//! Only what is unpublished is read: a blob below an epoch's trim point
//! holds nothing a shard still needs, so the index no longer names it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use oss_format::{
    NodeLogEntry, NodeLogHeader, OrdinalRange, ShardKey, Slice, UNASSIGNED_SEQ_NO, WalRecord,
    slice_records, trim_below,
};

use super::{NodeLogTakeover, TakeoverError};
use crate::log::{LogError, NodeLogStore, Position, lease_slot};

/// How a reader keeps bodies it read whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyCache {
    /// The largest body read whole, rather than slice by slice.
    pub max_body: u64,
    /// The most bytes of bodies kept at once.
    pub max_kept: u64,
    /// How long bodies are kept from the first one read: a takeover's
    /// opens read them within seconds of each other.
    pub keep_for: Duration,
}

impl Default for BodyCache {
    /// 8 MiB a body, 64 MiB in all, for 60 s (the Java's).
    fn default() -> Self {
        Self {
            max_body: 8 << 20,
            max_kept: 64 << 20,
            keep_for: Duration::from_secs(60),
        }
    }
}

/// One body: filled by the reader that claimed it, `None` when its read
/// failed and each shard reads its own slices instead.
type Kept = Arc<Mutex<Option<Arc<Vec<u8>>>>>;

/// Bodies read whole, per epoch and slot, within the cache's bounds.
pub(super) struct Bodies {
    cache: BodyCache,
    kept: BTreeMap<(u64, u64), Kept>,
    bytes: u64,
    since: Option<Instant>,
}

impl Bodies {
    pub(super) fn new(cache: BodyCache) -> Self {
        Self {
            cache,
            // ALLOC: one entry a body kept, within the cache's byte bound.
            kept: BTreeMap::new(),
            bytes: 0,
            since: None,
        }
    }
}

/// A record handed to a consumer, which may fail with its own error.
type Consumer<'f, E> = dyn FnMut(WalRecord<'_>) -> Result<(), E> + 'f;

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl NodeLogTakeover {
    /// The highest sequence number of a shard's records here, reading the
    /// header of each blob that holds them and no slice;
    /// [`UNASSIGNED_SEQ_NO`] when none is held.
    ///
    /// # Errors
    ///
    /// When a fence or a read fails.
    pub fn max_seq_no_of(&self, shard: &ShardKey) -> Result<i64, TakeoverError> {
        let fenced = self.fenced()?;
        let mut max = UNASSIGNED_SEQ_NO;
        for entry in &self.epochs {
            let epoch = lease_slot(entry.epoch);
            let Some(Some(last)) = fenced.last_headers.get(&epoch) else {
                continue;
            };
            for run in last.ordinals_of(shard) {
                for ordinal in run.from..=run.to {
                    if let Some(header) = self.header(epoch, ordinal, Some(last))? {
                        for slice in header.slices_of(shard) {
                            max = max.max(slice.max_seq_no);
                        }
                    }
                }
            }
        }
        Ok(max)
    }

    /// Whether any epoch's last header names unpublished records of a
    /// shard, reading no slice. A header's index is as of when it was
    /// written, so this errs towards a replay, never away from one.
    ///
    /// # Errors
    ///
    /// When a fence or a read fails.
    pub fn holds(&self, shard: &ShardKey) -> Result<bool, TakeoverError> {
        let fenced = self.fenced()?;
        Ok(fenced
            .last_headers
            .values()
            .flatten()
            .any(|last| !last.ordinals_of(shard).is_empty()))
    }

    /// Hands a shard's unpublished records from every fenced epoch to
    /// `each`, oldest first, in the order they replay.
    ///
    /// # Errors
    ///
    /// When a fence or a read fails, or `each` does.
    pub fn for_each_record<E: From<TakeoverError>>(
        &self,
        shard: &ShardKey,
        mut each: impl FnMut(WalRecord<'_>) -> Result<(), E>,
    ) -> Result<(), E> {
        self.records_from(shard, None, None, &mut each)
    }

    /// As [`NodeLogTakeover::for_each_record`], leaving out what landed at
    /// or before `after`: for a reader holding a commit that covers every
    /// record of the shard up to it.
    ///
    /// # Errors
    ///
    /// When a fence or a read fails, or `each` does.
    pub fn for_each_record_after<E: From<TakeoverError>>(
        &self,
        shard: &ShardKey,
        after: Position,
        mut each: impl FnMut(WalRecord<'_>) -> Result<(), E>,
    ) -> Result<(), E> {
        self.records_from(shard, Some(after), None, &mut each)
    }

    fn records_from<E: From<TakeoverError>>(
        &self,
        shard: &ShardKey,
        after: Option<Position>,
        through: Option<i64>,
        each: &mut Consumer<'_, E>,
    ) -> Result<(), E> {
        let fenced = self.fenced()?;
        for entry in &self.epochs {
            let epoch = lease_slot(entry.epoch);
            if after.is_some_and(|a| epoch < a.epoch) {
                continue;
            }
            let Some(Some(last)) = fenced.last_headers.get(&epoch) else {
                continue;
            };
            let runs = last.ordinals_of(shard);
            match after.filter(|a| a.epoch == epoch) {
                Some(a) => {
                    let later = trim_below(runs, a.ordinal.saturating_add(1));
                    self.read_runs(epoch, &later, Some(last), shard, through, each)?;
                }
                None => self.read_runs(epoch, runs, Some(last), shard, through, each)?,
            }
        }
        Ok(())
    }

    /// As [`NodeLogTakeover::for_each_record`], leaving out every slice
    /// whose highest sequence number for the shard is at or below
    /// `through`: for a successor whose commit holds every operation to
    /// there. A skipped slice is not fetched, and a blob with none left for
    /// the shard costs no body or slice read. A slice is judged by its
    /// highest sequence number, so one holding only unsequenced records is
    /// never skipped, and one that mixes them with sequenced records is
    /// judged by the sequenced ones: a node's log writers always assign
    /// identity. One handed over comes whole, records the commit holds
    /// included.
    ///
    /// # Errors
    ///
    /// When a fence or a read fails, or `each` does.
    pub fn for_each_record_above<E: From<TakeoverError>>(
        &self,
        shard: &ShardKey,
        through: i64,
        mut each: impl FnMut(WalRecord<'_>) -> Result<(), E>,
    ) -> Result<(), E> {
        self.records_from(shard, None, Some(through), &mut each)
    }

    /// Hands a shard's records in known runs of one epoch to `each`, with
    /// no fence: for a node reading its own log, which knows what it landed
    /// and where.
    ///
    /// # Errors
    ///
    /// When a read fails, or `each` does.
    pub fn for_each_record_in<E: From<TakeoverError>>(
        &self,
        epoch: u64,
        runs: &[OrdinalRange],
        shard: &ShardKey,
        mut each: impl FnMut(WalRecord<'_>) -> Result<(), E>,
    ) -> Result<(), E> {
        self.read_runs(epoch, runs, None, shard, None, &mut each)
    }

    fn read_runs<E: From<TakeoverError>>(
        &self,
        epoch: u64,
        runs: &[OrdinalRange],
        last: Option<&Arc<NodeLogHeader<'static>>>,
        shard: &ShardKey,
        through: Option<i64>,
        each: &mut Consumer<'_, E>,
    ) -> Result<(), E> {
        for run in runs {
            for ordinal in run.from..=run.to {
                let Some(header) = self.header(epoch, ordinal, last)? else {
                    continue;
                };
                // A slice whose highest sequence number the commit covers
                // holds nothing it lacks. An unassigned one is unknown.
                let wanted = |slice: &&Slice| {
                    through.is_none_or(|t| slice.max_seq_no < 0 || slice.max_seq_no > t)
                };
                if header.slices_of(shard).find(wanted).is_none() {
                    continue;
                }
                let body = if header.slices.len() > 1 {
                    self.body(epoch, &header)?
                } else {
                    None
                };
                for slice in header.slices_of(shard).filter(wanted) {
                    let read;
                    let bytes = match &body {
                        Some(body) => {
                            NodeLogStore::parse_slice(body, slice).map_err(TakeoverError::Log)?
                        }
                        None => {
                            read = self
                                .log
                                .read_slice(epoch, &header, slice)
                                .map_err(TakeoverError::Log)?;
                            read.as_slice()
                        }
                    };
                    for record in slice_records(bytes) {
                        let record = record.map_err(|e| TakeoverError::Log(LogError::Format(e)))?;
                        each(record)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The header of a blob, read once and kept; `None` when the index
    /// named a blob whose append's outcome was unknown and it did not land.
    fn header(
        &self,
        epoch: u64,
        ordinal: u64,
        last: Option<&Arc<NodeLogHeader<'static>>>,
    ) -> Result<Option<Arc<NodeLogHeader<'static>>>, TakeoverError> {
        if let Some(last) = last.filter(|l| l.ordinal == ordinal) {
            return Ok(Some(Arc::clone(last)));
        }
        if let Some(kept) = locked(&self.headers).get(&(epoch, ordinal)) {
            return Ok(Some(Arc::clone(kept)));
        }
        match self.log.read_entry(epoch, ordinal)? {
            None => Ok(None),
            Some(NodeLogEntry::Records(header)) => {
                let header = Arc::new(header);
                locked(&self.headers).insert((epoch, ordinal), Arc::clone(&header));
                Ok(Some(header))
            }
            Some(NodeLogEntry::Fence(_)) => Err(TakeoverError::Log(LogError::Format(
                oss_format::FormatError::Corrupt {
                    what: "node-log epoch",
                    reason: "a fence below its last header",
                },
            ))),
        }
    }

    /// A blob's body read whole and kept for the other shards in it, or
    /// `None` to read it slice by slice: too large, past what is kept, or
    /// its reader failed.
    fn body(
        &self,
        epoch: u64,
        header: &NodeLogHeader<'static>,
    ) -> Result<Option<Arc<Vec<u8>>>, TakeoverError> {
        let length = header.body_length();
        let key = (epoch, header.ordinal);
        // ALLOC: one slot a body kept, within the cache's byte bound.
        let kept: Kept = Arc::new(Mutex::new(None));
        let mut slot = {
            let mut bodies = locked(&self.bodies);
            let now = Instant::now();
            let cache = bodies.cache;
            let expired = bodies.since.is_some_and(|since| {
                !now.saturating_duration_since(since)
                    .saturating_sub(cache.keep_for)
                    .is_zero()
            });
            if expired {
                // Long past the takeover that read them: let them go.
                *bodies = Bodies::new(cache);
            }
            if let Some(other) = bodies.kept.get(&key).map(Arc::clone) {
                drop(bodies);
                // Its reader holds it until the read ends: `None` means the
                // read failed, and this one reads its own slices.
                return Ok(locked(&other).clone());
            }
            let total = bodies.bytes.saturating_add(length);
            if length > cache.max_body || total > cache.max_kept {
                return Ok(None);
            }
            // Claimed before the map is let go, so the others wait on it.
            let slot = locked(&kept);
            bodies.kept.insert(key, Arc::clone(&kept));
            bodies.bytes = total;
            bodies.since.get_or_insert(now);
            slot
        };
        match self.log.read_body(epoch, header) {
            Ok(body) => {
                let body = Arc::new(body);
                *slot = Some(Arc::clone(&body));
                Ok(Some(body))
            }
            Err(e) => {
                drop(slot);
                let mut bodies = locked(&self.bodies);
                if bodies.kept.get(&key).is_some_and(|k| Arc::ptr_eq(k, &kept)) {
                    bodies.kept.remove(&key);
                    bodies.bytes = bodies.bytes.saturating_sub(length);
                }
                Err(TakeoverError::Log(e))
            }
        }
    }

    /// Reads bodies whole within `cache`'s bounds rather than the
    /// defaults, dropping what is kept.
    #[must_use]
    pub fn with_body_cache(self, cache: BodyCache) -> Self {
        *locked(&self.bodies) = Bodies::new(cache);
        self
    }

    /// Bytes of bodies this reader keeps now.
    #[must_use]
    pub fn kept_body_bytes(&self) -> u64 {
        locked(&self.bodies).bytes
    }
}
