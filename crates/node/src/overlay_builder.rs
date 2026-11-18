//! Builds the overlay of the writes a node holds for a shard, off the search
//! path, and hands a search what is finished.
//!
//! A search asks [`OverlayBuilder::ready`], which never waits and never reads
//! the object store: it answers with the finished overlay when that covers
//! every record held, else queues one build on the worker thread and answers
//! with the last finished overlay over the same commit, or nothing. The
//! worker reserves the build's estimate from the `open_indexes` pool without
//! evicting an open index; a refusal serves the commit alone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use oss_format::WalRecord;
use oss_mem::{MemoryBudget, Pool, Reservation};
use oss_reader::commit_push::Stage;
use oss_reader::overlay::{
    CommitAccess, OverlaySource, Ready, build_ready, estimate_bytes, local_checkpoint,
};
use oss_transport::actions::tail::Record;

use crate::tail_held::TailHeld;

/// How many builds wait for the worker.
pub const QUEUE: usize = 64;

type Key = (String, u32);

/// A finished overlay, the records it covers and what it holds of the pool.
struct Built {
    ready: Arc<Ready>,
    through: u64,
    _charge: Reservation,
}

/// What the builder keeps for one shard.
struct Slot {
    stage: Stage,
    built: Option<Built>,
    /// The ordinal the latest queued or tried build covered up to: asked
    /// again only once more records are held.
    attempted: Option<u64>,
    /// The bytes the pool refused the latest build: tried again once the
    /// pool has that much free, though no record changed.
    refused: Option<usize>,
}

struct Job {
    key: Key,
    stage: Stage,
    commit: Arc<dyn CommitAccess>,
}

#[derive(Default)]
struct Control {
    pending: usize,
    paused: bool,
}

struct Inner {
    held: Arc<TailHeld>,
    budget: MemoryBudget,
    slots: Mutex<HashMap<Key, Slot>>,
    control: Mutex<Control>,
    changed: Condvar,
    builds: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An [`OverlaySource`] over the writes a node holds.
pub struct OverlayBuilder {
    inner: Arc<Inner>,
    queue: SyncSender<Job>,
}

impl OverlayBuilder {
    /// A builder over `held`, charging `budget`, with a queue of [`QUEUE`].
    pub fn new(held: Arc<TailHeld>, budget: MemoryBudget) -> Arc<Self> {
        Self::with_queue(held, budget, QUEUE)
    }

    /// As [`Self::new`] with a queue of `queue` builds.
    pub fn with_queue(held: Arc<TailHeld>, budget: MemoryBudget, queue: usize) -> Arc<Self> {
        let inner = Arc::new(Inner {
            held,
            budget,
            slots: Mutex::new(HashMap::new()),
            control: Mutex::new(Control::default()),
            changed: Condvar::new(),
            builds: AtomicU64::new(0),
        });
        let (tx, rx) = sync_channel(queue);
        let worker = Arc::clone(&inner);
        // The thread ends when the builder, and with it the sender, is dropped.
        std::thread::Builder::new()
            .name("overlay-builder".to_owned())
            .spawn(move || worker.run(&rx))
            .ok();
        Arc::new(Self { inner, queue: tx })
    }

    /// How many builds reached the engine, finished or not.
    pub fn builds(&self) -> u64 {
        self.inner.builds.load(Ordering::Relaxed)
    }

    /// How many builds are queued or running.
    pub fn pending(&self) -> usize {
        lock(&self.inner.control).pending
    }

    /// Holds the worker before its next build; queued builds wait.
    pub fn pause(&self) {
        lock(&self.inner.control).paused = true;
    }

    /// Lets the worker go on.
    pub fn resume(&self) {
        lock(&self.inner.control).paused = false;
        self.inner.changed.notify_all();
    }

    /// Waits until no build is queued or running.
    pub fn settle(&self) {
        let mut control = lock(&self.inner.control);
        while control.pending > 0 {
            control = self
                .inner
                .changed
                .wait(control)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl OverlaySource for OverlayBuilder {
    fn ready(
        &self,
        index_uuid: &str,
        shard: u32,
        stage: &Stage,
        commit: &Arc<dyn CommitAccess>,
        now_millis: u64,
    ) -> Option<Arc<Ready>> {
        let inner = &self.inner;
        let now = i64::try_from(now_millis).unwrap_or(i64::MAX);
        // ALLOC: one key a shard asked about, bounded by the open indexes.
        let key = (index_uuid.to_owned(), shard);
        let mut slots = lock(&inner.slots);
        let fresh = slots.get(&key).is_none_or(|slot| slot.stage != *stage);
        if fresh {
            if slots.contains_key(&key) {
                inner.held.untrack(index_uuid, shard, now);
            }
            inner
                .held
                .track(index_uuid, shard, local_checkpoint(commit.reader()));
            slots.insert(
                key.clone(),
                Slot {
                    // ALLOC: one stage a shard.
                    stage: stage.clone(),
                    built: None,
                    attempted: None,
                    refused: None,
                },
            );
        }
        let next = inner.held.next_ordinal(index_uuid, shard);
        let slot = slots.get_mut(&key)?;
        if let Some(built) = &slot.built
            && built.through == next
        {
            return Some(Arc::clone(&built.ready));
        }
        let freed = slot
            .refused
            .is_some_and(|bytes| inner.budget.available(Pool::OpenIndexes) >= bytes);
        if next > 0 && (slot.attempted != Some(next) || freed) {
            let job = Job {
                key,
                stage: stage.clone(),
                commit: Arc::clone(commit),
            };
            // Counted before the send: the worker may finish it first.
            {
                let mut control = lock(&inner.control);
                control.pending = control.pending.saturating_add(1);
            }
            match self.queue.try_send(job) {
                Ok(()) => {
                    slot.attempted = Some(next);
                    slot.refused = None;
                }
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => inner.finish(),
            }
        }
        slot.built.as_ref().map(|b| Arc::clone(&b.ready))
    }

    fn closed(&self, index_uuid: &str, shard: u32, stage: &Stage, now_millis: u64) {
        let now = i64::try_from(now_millis).unwrap_or(i64::MAX);
        let key = (index_uuid.to_owned(), shard);
        let mut slots = lock(&self.inner.slots);
        if slots.get(&key).is_some_and(|slot| slot.stage == *stage) {
            slots.remove(&key);
            self.inner.held.untrack(index_uuid, shard, now);
        }
    }
}

impl Inner {
    /// One build less pending.
    fn finish(&self) {
        let mut control = lock(&self.control);
        control.pending = control.pending.saturating_sub(1);
        self.changed.notify_all();
    }

    fn run(&self, queue: &Receiver<Job>) {
        while let Ok(job) = queue.recv() {
            {
                let mut control = lock(&self.control);
                while control.paused {
                    control = self
                        .changed
                        .wait(control)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
            self.build(&job);
            self.finish();
        }
    }

    fn build(&self, job: &Job) {
        let (uuid, shard) = (&job.key.0, job.key.1);
        let held = self.held.from(uuid, shard, 0);
        let Some(through) = held.last().map(|(ordinal, _)| ordinal.saturating_add(1)) else {
            return;
        };
        // ALLOC: one record a held record, at most MAX_RECORDS; borrowed text.
        let records: Vec<WalRecord<'_>> = held.iter().map(|(_, r)| wal(r)).collect();
        let estimate = estimate_bytes(&records);
        // Never evicts an open index: an overlay is worth less than one.
        let Ok(charge) = self.budget.try_reserve(Pool::OpenIndexes, estimate) else {
            if let Some(slot) = lock(&self.slots).get_mut(&job.key)
                && slot.stage == job.stage
            {
                slot.refused = Some(estimate);
            }
            return;
        };
        self.builds.fetch_add(1, Ordering::Relaxed);
        let Ok(ready) = build_ready(
            job.commit.reader(),
            job.commit.mapping_json(),
            &records,
            estimate,
        ) else {
            return;
        };
        let mut slots = lock(&self.slots);
        if let Some(slot) = slots.get_mut(&job.key)
            && slot.stage == job.stage
        {
            slot.built = Some(Built {
                ready: Arc::new(ready),
                through,
                _charge: charge,
            });
        }
    }
}

fn wal(r: &Record) -> WalRecord<'_> {
    if r.deletion {
        WalRecord::deletion(r.id.as_str(), r.seq_no, r.primary_term, r.version)
    } else {
        WalRecord::index(
            r.id.as_str(),
            r.source.as_str(),
            r.seq_no,
            r.primary_term,
            r.version,
        )
    }
}
