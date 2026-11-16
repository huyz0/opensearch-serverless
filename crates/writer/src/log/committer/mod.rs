//! Group commit across every shard of a node: whatever the node logs in one
//! window is one PUT to its write log (the Java's `NodeLogCommitter`).
//!
//! **The window.** The first record to arrive opens a window, and every
//! record that arrives before it closes rides the same blob, whichever
//! shard it is for. A window closes when its time is up or when it is full,
//! and a caller returns once the blob holding its records has landed, so a
//! write is durable before it is acknowledged.
//!
//! **One blob in flight.** Ordinals have no gaps, which is what lets a
//! successor fence by taking the first free slot. So the next window is
//! written only once the last has landed, and records keep joining it.
//!
//! **Epochs.** Before the first blob of an epoch, the epoch is recorded in
//! this node's lease ([`EpochLease`]), so a successor that reads the lease
//! knows to fence it. A failed append ends the epoch, whatever the failure:
//! a fence means a successor closed it, and an unknown outcome leaves a slot
//! this writer cannot reason about. The next window opens a new epoch.
//!
//! **Ownership.** Before a shard's first records go into an epoch,
//! [`ShardAdmission`] confirms this node still owns the shard at that term,
//! after the epoch is in the lease. A successor moves a shard's head and
//! then fences every epoch in this node's lease, so either the check sees
//! the move, or the successor's fence closes the epoch to this writer.
//!
//! **Trimming.** Per epoch, where each shard's unpublished records are. An
//! epoch's trim point is the lowest of them; an epoch no longer written and
//! holding nothing unpublished is complete, deletable, and leaves the lease.

mod join;
mod track;
mod types;
mod write;

pub(crate) use types::epoch_entry;
pub use types::{CommitError, CommitterConfig, EpochLease, Position, Refusal, ShardAdmission};

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use oss_format::{ShardKey, WalRecord};

use super::NodeLogStore;
use super::epochs::EpochState;
use super::hedge::Spawn;
use super::hedge_delay::HedgeDelay;
use super::window::{Latch, Window, WindowResult};
use join::encode_all;

/// The committer's state, under its one lock.
pub(crate) struct Inner {
    windows: VecDeque<Window>,
    pub(crate) epochs: BTreeMap<u64, EpochState>,
    flushing: bool,
    /// The epoch being written; `None` when the next window opens one.
    pub(crate) current: Option<u64>,
    /// An epoch being recorded in the lease, named by `positions` already.
    pub(crate) opening: Option<u64>,
    /// The epoch the next opened one takes.
    pub(crate) next_epoch: u64,
    last_flush_started: Option<Instant>,
}

/// Group commit for one node's write log.
pub struct NodeLogCommitter {
    pub(crate) store: NodeLogStore,
    pub(crate) lease: Box<dyn EpochLease>,
    pub(crate) admission: Box<dyn ShardAdmission>,
    config: CommitterConfig,
    pub(crate) inner: Mutex<Inner>,
    changed: Condvar,
    idle_flush: std::sync::atomic::AtomicBool,
    pub(crate) hedge_delay: HedgeDelay,
    pub(crate) hedge_spawn: Mutex<Option<Arc<dyn Spawn>>>,
    pub(crate) blobs: AtomicU64,
    pub(crate) records: AtomicU64,
    pub(crate) hedges: AtomicU64,
    pub(crate) hedges_won: AtomicU64,
}

impl fmt::Debug for NodeLogCommitter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeLogCommitter")
            .field("store", &self.store)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl NodeLogCommitter {
    /// A committer writing `store` from `first_epoch`, above every epoch
    /// this node wrote before.
    pub fn new(
        store: NodeLogStore,
        first_epoch: u64,
        lease: Box<dyn EpochLease>,
        admission: Box<dyn ShardAdmission>,
        config: CommitterConfig,
    ) -> Self {
        Self {
            store,
            lease,
            admission,
            config: CommitterConfig {
                max_bytes: config.max_bytes.max(1),
                max_records: config.max_records.max(1),
                ..config
            },
            inner: Mutex::new(Inner {
                // ALLOC: the open windows: those writers wait on, and those enqueue fills between flushes.
                windows: VecDeque::new(),
                // ALLOC: one state an open epoch, a handful.
                epochs: BTreeMap::new(),
                flushing: false,
                current: None,
                opening: None,
                next_epoch: first_epoch,
                last_flush_started: None,
            }),
            changed: Condvar::new(),
            idle_flush: std::sync::atomic::AtomicBool::new(false),
            hedge_delay: HedgeDelay::default(),
            hedge_spawn: Mutex::new(None),
            blobs: AtomicU64::new(0),
            records: AtomicU64::new(0),
            hedges: AtomicU64::new(0),
            hedges_won: AtomicU64::new(0),
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Logs a shard's records durably, in the same blob as whatever else
    /// the node logs in this window, and returns where they landed; `None`
    /// for no records.
    ///
    /// # Errors
    ///
    /// [`CommitError`] if the blob carrying them did not land, or the shard
    /// was refused admission to the epoch.
    pub fn commit(
        &self,
        shard: &ShardKey,
        term: i64,
        records: &[WalRecord<'_>],
    ) -> Result<Option<Position>, CommitError> {
        if records.is_empty() {
            return Ok(None);
        }
        let counted = encode_all(records)?;
        let mut inner = self.lock();
        let joined = self.join_all(&mut inner, shard, term, counted);
        let Some(mine) = joined.last().cloned() else {
            return Err(CommitError::Lost);
        };
        // The last window joined is the youngest: whether a writer already
        // drives it is on it there.
        let driven = inner
            .windows
            .back_mut()
            .filter(|w| Arc::ptr_eq(&w.latch, &mine))
            .is_some_and(|w| std::mem::replace(&mut w.driven, true));
        if driven {
            // Another writer will write it, or the windows before it, when
            // due: wait for it to land without the lock.
            drop(inner);
            let patience = self
                .config
                .window
                .saturating_mul(4)
                .max(Duration::from_secs(1));
            if mine.wait(patience).is_none() {
                // Its driver left it: drive it here.
                inner = self.lock();
                inner = self.await_landed(inner, &mine);
                drop(inner);
            }
        } else {
            inner = self.await_landed(inner, &mine);
            drop(inner);
        }
        // Windows are written in order, one at a time, so the last landing
        // means the others were written.
        for latch in &joined {
            let result = latch.get().ok_or(CommitError::Lost)?;
            if let Some(failure) = result.failures.get(shard) {
                return Err(failure.clone());
            }
        }
        mine.get()
            .and_then(|r| r.landed)
            .map(Some)
            .ok_or(CommitError::Lost)
    }

    /// Adds a shard's records to the window now open without waiting for it
    /// to land: they ride whatever is logged next. `on_landed` is told where
    /// they landed, after their window is written and without the committer's
    /// lock; it is not told if they did not land.
    ///
    /// # Errors
    ///
    /// [`CommitError::Encode`] if a record cannot be encoded.
    pub fn enqueue(
        &self,
        shard: &ShardKey,
        term: i64,
        records: &[WalRecord<'_>],
        on_landed: impl FnOnce(Position) + Send + 'static,
    ) -> Result<(), CommitError> {
        if records.is_empty() {
            return Ok(());
        }
        let counted = encode_all(records)?;
        let mut inner = self.lock();
        let joined = self.join_all(&mut inner, shard, term, counted);
        let shard = shard.clone();
        if let Some(window) = inner.windows.back_mut() {
            window
                .on_landed
                // ALLOC: one callback a record set enqueued, run once its window is written.
                .push(Box::new(move |result: &WindowResult| {
                    let failed = joined
                        .iter()
                        .any(|latch| latch.get().is_none_or(|r| r.failures.contains_key(&shard)));
                    // `joined` ends with this window, so its failures are
                    // among those read above.
                    if let (Some(at), false) = (result.landed, failed) {
                        on_landed(at);
                    }
                }));
        }
        Ok(())
    }

    /// Writes what [`NodeLogCommitter::enqueue`] left in the open windows,
    /// so records nothing else carries land: one write on a node that
    /// logged nothing since, none on a node that is logging.
    pub fn flush_queued(&self) {
        let inner = self.lock();
        let latch = inner
            .windows
            .back()
            .filter(|w| !w.on_landed.is_empty())
            .map(|w| Arc::clone(&w.latch));
        if let Some(latch) = latch {
            drop(self.await_landed(inner, &latch));
        }
    }

    /// Paces windows by the last blob instead of by their first record: a
    /// record finding the log quiet for a whole window is written at once.
    /// Costs PUTs: up to about half as many blobs again at one record a
    /// window.
    pub fn flush_when_idle(&self, on: bool) {
        self.idle_flush.store(on, Ordering::Relaxed);
    }

    /// Sends a window's PUT again when the first has not answered within
    /// `after`, raised to the 95th percentile of the last 128 PUTs and
    /// scaled by the blob's size; zero or no `spawn` never sends a second.
    pub fn hedge_appends(&self, after: Duration, spawn: Option<Arc<dyn Spawn>>) {
        self.hedge_delay.configure(after);
        *self
            .hedge_spawn
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = spawn;
    }

    /// How long a PUT of `blob_bytes` waits before it is sent again; zero
    /// when hedging is off.
    pub fn hedge_delay(&self, blob_bytes: u64) -> Duration {
        self.hedge_delay.delay(blob_bytes)
    }

    /// Blobs landed: the PUTs spent on records, hedges aside.
    pub fn blobs(&self) -> u64 {
        self.blobs.load(Ordering::Relaxed)
    }

    /// Records those blobs carried.
    pub fn records_committed(&self) -> u64 {
        self.records.load(Ordering::Relaxed)
    }

    /// Windows whose PUT was sent a second time, and of those, the ones the
    /// second answered first.
    pub fn hedges(&self) -> (u64, u64) {
        (
            self.hedges.load(Ordering::Relaxed),
            self.hedges_won.load(Ordering::Relaxed),
        )
    }

    /// Lingers on, or writes, windows until `mine` is done. Called and
    /// returns with the lock held.
    fn await_landed<'a>(
        &'a self,
        mut inner: MutexGuard<'a, Inner>,
        mine: &Arc<Latch>,
    ) -> MutexGuard<'a, Inner> {
        while mine.get().is_none() {
            if inner.flushing {
                inner = self
                    .changed
                    .wait(inner)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            let Some(head) = inner.windows.front() else {
                // Not in flight and not waiting: nothing can land it.
                break;
            };
            // Due once full or out of time; else it lingers for the rest.
            let full = head.full(self.config.max_bytes, self.config.max_records);
            let left = head
                .closes_at
                .checked_duration_since(Instant::now())
                .filter(|left| !left.is_zero());
            if let (false, Some(left)) = (full, left) {
                inner = self
                    .changed
                    .wait_timeout(inner, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
                continue;
            }
            let Some(window) = inner.windows.pop_front() else {
                break;
            };
            let mut head = Flushing {
                window,
                landed: None,
            };
            inner.flushing = true;
            inner.last_flush_started = Some(Instant::now());
            drop(inner);
            let wrote =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.write(&mut head)));
            if wrote.is_err() {
                // Its outcome is unknown: nobody in the window may read it
                // as landed, and the epoch is closed to this writer.
                head.landed = None;
                head.window.fail_all(&CommitError::Lost);
                self.abandon(&head.window);
            }
            inner = self.lock();
            inner.flushing = false;
            let on_landed = std::mem::take(&mut head.window.on_landed);
            let result = WindowResult {
                landed: head.landed,
                failures: std::mem::take(&mut head.window.failures),
            };
            head.window.latch.set(result);
            self.changed.notify_all();
            // The callbacks run without the lock, so a slow one holds up
            // no writer.
            drop(inner);
            if let Some(result) = head.window.latch.get() {
                for each in on_landed {
                    // Bookkeeping only: nothing that wrote them waits on it.
                    let _ =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| each(&result)));
                }
            }
            if let Err(panic) = wrote {
                std::panic::resume_unwind(panic);
            }
            inner = self.lock();
        }
        inner
    }
}

/// A window being written, owned by its writer.
pub(crate) struct Flushing {
    pub(crate) window: Window,
    pub(crate) landed: Option<Position>,
}
