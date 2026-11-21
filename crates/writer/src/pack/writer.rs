//! Gathers the segment blobs a node's shards publish at about the same time
//! into one node pack, so the node pays one PUT for all of them.
//!
//! Ported from `NodePackWriter` (huyz0/OpenSearch
//! `feature/serverlessshell`). A publish hands its blobs to
//! [`PackWriter::add`] and waits for the pack to land. The first blob into an
//! empty batch makes its caller the batch's leader: it waits up to the
//! linger, or until the batch holds `max_pack_bytes`, then takes the batch,
//! writes it as one pack and tells every waiter where its blob landed. Blobs
//! arriving while a pack is written start the next batch. No thread of its
//! own: the publishing threads do the work, and a node whose publishes never
//! overlap writes one pack a publish, the PUT count it had without packs.
//!
//! Each writer has its own epoch, so a restarted node never reuses a name.
//! Ordinals count up from zero and are spent even by a pack whose write
//! failed; a gap costs nothing, since nothing reads packs by range.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::PackError;
use super::ledger::PackLedger;
use super::store::{Landed, PackStore, Part};

/// How long a publish may take from its pack landing to its manifest swap.
/// Past it the swap is refused, which is what lets collection delete a pack
/// no manifest names once it is older than [`COLLECTABLE_AFTER_MILLIS`].
pub const SWAP_DEADLINE_MILLIS: u64 = 5 * 60 * 1000;

/// How old a pack must be, since collection first saw it, before collection
/// may judge it unreferenced.
pub const COLLECTABLE_AFTER_MILLIS: u64 = 3 * SWAP_DEADLINE_MILLIS;

type Answer = Result<Landed, PackError>;

struct Pending {
    part: Part,
    answer: SyncSender<Answer>,
}

#[derive(Default)]
struct Batch {
    pending: Vec<Pending>,
    bytes: u64,
}

/// One node's pack writer, shared by every shard it publishes.
pub struct PackWriter {
    store: PackStore,
    ledger: Option<PackLedger>,
    epoch: u64,
    linger: Duration,
    max_pack_bytes: u64,
    next_ordinal: AtomicU64,
    recorded: Mutex<bool>,
    batch: Mutex<Batch>,
    full: Condvar,
}

impl std::fmt::Debug for PackWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackWriter")
            .field("epoch", &self.epoch)
            .field("next_ordinal", &self.next_ordinal)
            .finish_non_exhaustive()
    }
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PackWriter {
    /// A writer of `store`'s packs in `epoch`, which no earlier writer of the
    /// node used. The first blob of a batch waits `linger` for others; a
    /// batch of `max_pack_bytes` is written without waiting it out. With a
    /// `ledger`, the epoch is recorded in it before the first pack.
    pub fn new(
        store: PackStore,
        ledger: Option<PackLedger>,
        epoch: u64,
        linger: Duration,
        max_pack_bytes: u64,
    ) -> Self {
        Self {
            store,
            ledger,
            epoch,
            linger,
            max_pack_bytes: max_pack_bytes.max(1),
            next_ordinal: AtomicU64::new(0),
            recorded: Mutex::new(false),
            batch: Mutex::new(Batch::default()),
            full: Condvar::new(),
        }
    }

    /// This writer's epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The next ordinal this writer will use: every pack it has written, or
    /// tried to, is below it.
    pub fn next_ordinal(&self) -> u64 {
        self.next_ordinal.load(Ordering::Acquire)
    }

    /// Adds one shard's segment blobs to the next pack, all to the same one,
    /// and waits for the pack to land: where each landed, in order.
    ///
    /// # Errors
    ///
    /// The pack's failure; the blobs are then in no pack anyone can see.
    pub fn add(&self, parts: Vec<Part>) -> Result<Vec<Landed>, PackError> {
        if parts.is_empty() {
            // ALLOC: one entry a part of one publish window, or a key.
            return Ok(Vec::new());
        }
        // ALLOC: one receiver a part of one publish.
        let mut receivers: Vec<Receiver<Answer>> = Vec::with_capacity(parts.len());
        let leader;
        {
            let mut batch = locked(&self.batch);
            leader = batch.pending.is_empty();
            for part in parts {
                let (answer, receiver) = sync_channel(1);
                receivers.push(receiver);
                batch.bytes = batch.bytes.saturating_add(part.source.len());
                batch.pending.push(Pending { part, answer });
            }
            if batch.bytes >= self.max_pack_bytes {
                self.full.notify_all();
            }
        }
        let mut own_failure = None;
        if leader {
            own_failure = self.lead();
        }
        // ALLOC: one landing a part of one publish.
        let mut landed = Vec::with_capacity(receivers.len());
        for receiver in receivers {
            match receiver.recv() {
                Ok(Ok(one)) => landed.push(one),
                Ok(Err(e)) => return Err(own_failure.take().unwrap_or(e)),
                Err(_) => {
                    return Err(own_failure.take().unwrap_or_else(|| {
                        PackError::Shared("the pack's writer went away".into())
                    }));
                }
            }
        }
        Ok(landed)
    }

    /// Waits out the linger, takes the batch and writes it as one pack.
    /// Returns the failure, for the leader's own blobs.
    fn lead(&self) -> Option<PackError> {
        let taken = {
            let now = Instant::now();
            let deadline = now.checked_add(self.linger).unwrap_or(now);
            let mut batch = locked(&self.batch);
            while batch.bytes < self.max_pack_bytes {
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    break;
                };
                batch = self
                    .full
                    .wait_timeout(batch, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            std::mem::take(&mut *batch)
        };
        let ordinal = self.next_ordinal.fetch_add(1, Ordering::AcqRel);
        // ALLOC: one entry a part of one publish window, or a key.
        let mut answers = Vec::with_capacity(taken.pending.len());
        // ALLOC: one entry a part of one publish window, or a key.
        let mut parts = Vec::with_capacity(taken.pending.len());
        for pending in taken.pending {
            answers.push(pending.answer);
            parts.push(pending.part);
        }
        match self.write(ordinal, parts) {
            Ok(landed) => {
                for (answer, one) in answers.into_iter().zip(landed) {
                    let _ = answer.send(Ok(one));
                }
                None
            }
            Err(e) => {
                // ALLOC: one entry a part of one publish window, or a key.
                let why = e.to_string();
                for answer in answers {
                    let _ = answer.send(Err(PackError::Shared(why.clone())));
                }
                Some(e)
            }
        }
    }

    fn write(&self, ordinal: u64, parts: Vec<Part>) -> Result<Vec<Landed>, PackError> {
        self.record_epoch()?;
        self.store.write(self.epoch, ordinal, parts)
    }

    fn record_epoch(&self) -> Result<(), PackError> {
        let Some(ledger) = &self.ledger else {
            return Ok(());
        };
        let mut recorded = locked(&self.recorded);
        if !*recorded {
            ledger.add_epoch(self.epoch)?;
            *recorded = true;
        }
        Ok(())
    }
}
