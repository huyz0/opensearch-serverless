//! Builds the overlay of the writes a node holds for a shard, off the search
//! path, and hands a search what is finished.
//!
//! A search asks [`OverlayBuilder::ready`], which never waits and never reads
//! the object store: it answers with the finished overlay when that covers
//! every record held, else queues one build on the worker thread and answers
//! with the last finished overlay over the same commit, or nothing. The
//! worker reserves the build's estimate from the `open_indexes` pool without
//! evicting an open index; a refusal serves the commit alone.
//!
//! A builder with a [`TailFetcher`] also asks the writer a shard's commit
//! names, once for each stage, for the records past the commit's checkpoint,
//! on the same worker, before that stage's first build.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use oss_format::WalRecord;
use oss_mem::{MemoryBudget, Pool, Reservation};
use oss_reader::commit_push::Stage;
use oss_reader::overlay::{
    CommitAccess, OverlaySource, Ready, build_ready, estimate_bytes, kept_bytes, local_checkpoint,
    mask_bytes,
};
use oss_transport::actions::tail::{FetchRequest, FetchResponse, NEW, Record, ShardRef};
use oss_transport::stream::TaskId;

use crate::newest_ask::LiveWriters;
use crate::tail_held::TailHeld;
use crate::tail_reader_link::{Clock, TransportTailFetch};

/// Asks a writer for the records it acknowledged past a commit.
pub trait TailFetcher: Send + Sync {
    /// The writer `writer`'s answer to `request`, or nothing.
    fn fetch(&self, writer: &str, request: &FetchRequest) -> Option<FetchResponse>;
}

impl<L: LiveWriters> TailFetcher for TransportTailFetch<L> {
    fn fetch(&self, writer: &str, request: &FetchRequest) -> Option<FetchResponse> {
        TransportTailFetch::fetch(self, writer, request)
    }
}

/// What a builder fetches with, and as whom.
struct Fetching {
    fetcher: Arc<dyn TailFetcher>,
    subscriber: String,
    clock: Clock,
}

/// How many builds wait for the worker.
pub const QUEUE: usize = 64;

/// The longest [`OverlayBuilder::settle`] waits.
pub const SETTLE: std::time::Duration = std::time::Duration::from_secs(10);

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
    /// Whether to ask the stage's writer first.
    fetch: bool,
}

#[derive(Default)]
struct Control {
    pending: usize,
    paused: bool,
}

struct Inner {
    held: Arc<TailHeld>,
    budget: MemoryBudget,
    fetching: Option<Fetching>,
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
        Self::start(held, budget, queue, None)
    }

    /// As [`Self::new`], asking a shard's writer through `fetcher` for the
    /// records past the commit, as the node `subscriber`, timed by `clock`.
    pub fn fetching(
        held: Arc<TailHeld>,
        budget: MemoryBudget,
        fetcher: Arc<dyn TailFetcher>,
        subscriber: String,
        clock: Clock,
    ) -> Arc<Self> {
        let fetching = Fetching {
            fetcher,
            subscriber,
            clock,
        };
        Self::start(held, budget, QUEUE, Some(fetching))
    }

    fn start(
        held: Arc<TailHeld>,
        budget: MemoryBudget,
        queue: usize,
        fetching: Option<Fetching>,
    ) -> Arc<Self> {
        let inner = Arc::new(Inner {
            held,
            budget,
            fetching,
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

    /// Waits until no build is queued or running, at most [`SETTLE`];
    /// whether none is.
    pub fn settle(&self) -> bool {
        let started = std::time::Instant::now();
        let mut control = lock(&self.inner.control);
        while control.pending > 0 {
            let left = SETTLE.saturating_sub(started.elapsed());
            if left.is_zero() {
                return false;
            }
            control = self
                .inner
                .changed
                .wait_timeout(control, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl OverlayBuilder {
    /// Queues `job` for the worker; whether the queue took it.
    fn enqueue(&self, job: Job) -> bool {
        // Counted before the send: the worker may finish it first.
        {
            let mut control = lock(&self.inner.control);
            control.pending = control.pending.saturating_add(1);
        }
        match self.queue.try_send(job) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.inner.finish();
                false
            }
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
        if fresh && inner.fetching.is_some() && stage.writer().is_some() {
            self.enqueue(Job {
                key: key.clone(),
                stage: stage.clone(),
                commit: Arc::clone(commit),
                fetch: true,
            });
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
                fetch: false,
            };
            if self.enqueue(job) {
                slot.attempted = Some(next);
                slot.refused = None;
            }
        }
        slot.built.as_ref().map(|b| Arc::clone(&b.ready))
    }

    fn apply_pending(
        &self,
        index_uuid: &str,
        shard: u32,
        stage: &Stage,
        commit: &Arc<dyn CommitAccess>,
    ) {
        let inner = &self.inner;
        let next = inner.held.next_ordinal(index_uuid, shard);
        // ALLOC: one key a shard asked about, bounded by the open indexes.
        let key = (index_uuid.to_owned(), shard);
        let covers = |slots: &HashMap<Key, Slot>| {
            slots.get(&key).and_then(|slot| {
                (slot.stage == *stage).then(|| {
                    slot.built
                        .as_ref()
                        .is_some_and(|built| built.through >= next)
                })
            })
        };
        // A shard no engine tracks here, or at another stage, is not ours.
        if next == 0 || covers(&lock(&inner.slots)) != Some(false) {
            return;
        }
        inner.build(&Job {
            key: key.clone(),
            stage: stage.clone(),
            commit: Arc::clone(commit),
            fetch: false,
        });
        if covers(&lock(&inner.slots)) == Some(false) {
            // The overlay could not take every record: whoever answers must
            // not claim it holds them.
            inner.held.skipped(index_uuid, shard);
        }
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
            if job.fetch {
                self.fetch(&job);
            }
            self.build(&job);
            self.finish();
        }
    }

    /// Asks the writer the job's stage names for the records past the
    /// commit, and holds its answer.
    fn fetch(&self, job: &Job) {
        let Some(fetching) = &self.fetching else {
            return;
        };
        let Some(writer) = job.stage.writer() else {
            return;
        };
        let (uuid, shard) = (&job.key.0, job.key.1);
        let current = lock(&self.slots)
            .get(&job.key)
            .is_some_and(|slot| slot.stage == job.stage);
        if !current {
            return;
        }
        // ALLOC: one request a shard open, a few short strings.
        let request = FetchRequest {
            parent: TaskId::EMPTY,
            shard: ShardRef {
                index_name: job.commit.index_name().to_owned(),
                index_uuid: uuid.clone(),
                shard: i32::try_from(shard).unwrap_or(i32::MAX),
            },
            after_seq_no: local_checkpoint(job.commit.reader()),
            subscriber: fetching.subscriber.clone(),
            subscription: NEW,
        };
        let sent_at = (fetching.clock)();
        if let Some(answer) = fetching.fetcher.fetch(writer, &request) {
            self.held
                .answered(uuid, shard, writer, NEW, sent_at, &answer);
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
        // The view a search caches also holds a mask of every commit segment.
        let charged = estimate.saturating_add(mask_bytes(job.commit.reader()));
        let Ok(mut charge) = self.budget.try_reserve(Pool::OpenIndexes, charged) else {
            if let Some(slot) = lock(&self.slots).get_mut(&job.key)
                && slot.stage == job.stage
            {
                slot.refused = Some(charged);
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
        // The peak is over: the pool holds what the finished overlay keeps.
        let kept = kept_bytes(&records, ready.overlay().stored_bytes())
            .saturating_add(mask_bytes(job.commit.reader()));
        charge.shrink(charged.saturating_sub(kept));
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
