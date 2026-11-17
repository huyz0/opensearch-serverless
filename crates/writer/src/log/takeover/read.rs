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
    NodeLogEntry, NodeLogHeader, OrdinalRange, ShardKey, UNASSIGNED_SEQ_NO, WalRecord,
    slice_records, trim_below,
};

use super::{NodeLogTakeover, TakeoverError};
use crate::log::{LogError, NodeLogStore, Position, lease_slot};

/// The largest body read whole, rather than slice by slice.
pub const MAX_BODY_BYTES: u64 = 8 << 20;
/// The most bytes of bodies one reader keeps.
pub const MAX_KEPT_BYTES: u64 = 64 << 20;
/// How long bodies are kept from the first one read: a takeover's opens
/// read them within seconds of each other.
pub const KEEP_BODIES: Duration = Duration::from_secs(60);

/// One body: filled by the reader that claimed it, `None` when its read
/// failed and each shard reads its own slices instead.
type Kept = Arc<Mutex<Option<Arc<Vec<u8>>>>>;

/// Bodies read whole, per epoch and slot, within [`MAX_KEPT_BYTES`].
#[derive(Default)]
pub(super) struct Bodies {
    kept: BTreeMap<(u64, u64), Kept>,
    bytes: u64,
    since: Option<Instant>,
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
        self.records_from(shard, None, &mut each)
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
        self.records_from(shard, Some(after), &mut each)
    }

    fn records_from<E: From<TakeoverError>>(
        &self,
        shard: &ShardKey,
        after: Option<Position>,
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
                    self.read_runs(epoch, &later, Some(last), shard, each)?;
                }
                None => self.read_runs(epoch, runs, Some(last), shard, each)?,
            }
        }
        Ok(())
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
        self.read_runs(epoch, runs, None, shard, &mut each)
    }

    fn read_runs<E: From<TakeoverError>>(
        &self,
        epoch: u64,
        runs: &[OrdinalRange],
        last: Option<&Arc<NodeLogHeader<'static>>>,
        shard: &ShardKey,
        each: &mut Consumer<'_, E>,
    ) -> Result<(), E> {
        for run in runs {
            for ordinal in run.from..=run.to {
                let Some(header) = self.header(epoch, ordinal, last)? else {
                    continue;
                };
                if header.slices_of(shard).next().is_none() {
                    continue;
                }
                let body = if header.slices.len() > 1 {
                    self.body(epoch, &header)?
                } else {
                    None
                };
                for slice in header.slices_of(shard) {
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
        let (kept, mine) = {
            let mut bodies = locked(&self.bodies);
            let now = Instant::now();
            if bodies
                .since
                .is_some_and(|since| now.saturating_duration_since(since) > KEEP_BODIES)
            {
                // Long past the takeover that read them: let them go.
                *bodies = Bodies::default();
            }
            if let Some(kept) = bodies.kept.get(&key) {
                (Arc::clone(kept), None)
            } else {
                if length > MAX_BODY_BYTES || bodies.bytes.saturating_add(length) > MAX_KEPT_BYTES {
                    return Ok(None);
                }
                let kept: Kept = Arc::new(Mutex::new(None));
                // Claimed before the map is let go, so the others wait on it.
                let guard = locked(&kept);
                let claim = Arc::clone(&kept);
                bodies.kept.insert(key, Arc::clone(&kept));
                bodies.bytes = bodies.bytes.saturating_add(length);
                bodies.since.get_or_insert(now);
                drop(guard);
                (claim, Some(()))
            }
        };
        let mut slot = locked(&kept);
        if mine.is_none() || slot.is_some() {
            return Ok(slot.clone());
        }
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
}
