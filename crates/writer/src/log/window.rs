//! One window's records, by shard and term, and how writing them went.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use oss_blob::cost::{self, RequestCost, Scope};
use oss_format::{EncodedWalRecord, ShardKey};

use super::committer::{CommitError, Position};

/// What writing a window came to, set once.
#[derive(Debug, Default)]
pub(crate) struct WindowResult {
    /// Where its blob landed; `None` when nothing was written.
    pub(crate) landed: Option<Position>,
    /// The shards that did not land, and why.
    pub(crate) failures: BTreeMap<ShardKey, CommitError>,
    /// When the window was due to close.
    pub(crate) closes_at: Option<Instant>,
    /// When its write started, and when it ended.
    pub(crate) flush_started: Option<Instant>,
    pub(crate) done: Option<Instant>,
}

/// A window's result, waited for by its writers (the Java's `landedLatch`).
#[derive(Debug, Default)]
pub(crate) struct Latch {
    result: Mutex<Option<Arc<WindowResult>>>,
    set: Condvar,
}

impl Latch {
    fn slot(&self) -> std::sync::MutexGuard<'_, Option<Arc<WindowResult>>> {
        self.result.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The result, once set.
    pub(crate) fn get(&self) -> Option<Arc<WindowResult>> {
        self.slot().clone()
    }

    /// Sets the result and wakes every writer waiting on it.
    pub(crate) fn set(&self, result: WindowResult) {
        *self.slot() = Some(Arc::new(result));
        self.set.notify_all();
    }

    /// Waits up to `patience` for the result.
    pub(crate) fn wait(&self, patience: Duration) -> Option<Arc<WindowResult>> {
        let deadline = Instant::now().checked_add(patience);
        let mut slot = self.slot();
        loop {
            if let Some(result) = slot.as_ref() {
                return Some(Arc::clone(result));
            }
            let left = deadline.map_or(patience, |d| d.saturating_duration_since(Instant::now()));
            if left.is_zero() {
                return None;
            }
            slot = self
                .set
                .wait_timeout(slot, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// A record with the bytes of id and source a window counts for it.
#[derive(Debug)]
pub(crate) struct Counted {
    pub(crate) record: EncodedWalRecord,
    pub(crate) content: u64,
}

/// Told the result of the window a record was enqueued in.
pub(crate) type OnLanded = Box<dyn FnOnce(&WindowResult) + Send>;

/// One window (the Java's `NodeLogWindow`).
pub(crate) struct Window {
    pub(crate) closes_at: Instant,
    /// The batches, in the order they first joined.
    pub(crate) batches: Vec<((ShardKey, i64), Vec<EncodedWalRecord>)>,
    index: BTreeMap<(ShardKey, i64), usize>,
    pub(crate) failures: BTreeMap<ShardKey, CommitError>,
    pub(crate) bytes: u64,
    pub(crate) records: u64,
    /// Closed before its bounds because the records after it would not fit.
    pub(crate) close_early: bool,
    /// Whether a writer already waits on it under the lock, and so writes it.
    pub(crate) driven: bool,
    pub(crate) latch: Arc<Latch>,
    pub(crate) on_landed: Vec<OnLanded>,
    /// The writers that asked to see their cost, each with the bytes it put
    /// in, which is its share of the window's store requests.
    shares: Vec<(Arc<RequestCost>, u64)>,
}

impl Window {
    pub(crate) fn new(closes_at: Instant) -> Self {
        Self {
            closes_at,
            // ALLOC: one entry a (shard, term) in the window, bounded by its records.
            batches: Vec::new(),
            index: BTreeMap::new(),
            failures: BTreeMap::new(),
            bytes: 0,
            records: 0,
            close_early: false,
            driven: false,
            // ALLOC: one latch a window, shared by its writers.
            latch: Arc::new(Latch::default()),
            on_landed: Vec::new(),
            // ALLOC: one entry a writer that asked, bounded by the window's writers.
            shares: Vec::new(),
        }
    }

    pub(crate) fn add(&mut self, shard: &ShardKey, term: i64, more: Vec<Counted>) {
        let key = (shard.clone(), term);
        let at = match self.index.get(&key) {
            Some(at) => *at,
            None => {
                let at = self.batches.len();
                self.index.insert(key.clone(), at);
                // ALLOC: the batch's records, bounded by the window's.
                self.batches.push((key, Vec::with_capacity(more.len())));
                at
            }
        };
        let put_in = more
            .iter()
            .fold(0u64, |sum, c| sum.saturating_add(c.content))
            .max(1);
        if let Some(asked) = cost::capture() {
            match self.shares.iter_mut().find(|(c, _)| Arc::ptr_eq(c, &asked)) {
                Some((_, weight)) => *weight = weight.saturating_add(put_in),
                // ALLOC: one entry a writer that asked.
                None => self.shares.push((asked, put_in)),
            }
        }
        self.records = self
            .records
            .saturating_add(u64::try_from(more.len()).unwrap_or(u64::MAX));
        if let Some((_, batch)) = self.batches.get_mut(at) {
            for counted in more {
                self.bytes = self.bytes.saturating_add(counted.content);
                batch.push(counted.record);
            }
        }
    }

    /// What the store requests of writing this window count for: each
    /// asking writer's share by bytes of the whole window's. `None` when no
    /// writer asked, so the writer that drives it is charged nothing.
    pub(crate) fn scope(&self) -> Option<Scope> {
        if self.shares.is_empty() {
            return None;
        }
        Some(Scope::split(self.shares.clone(), self.bytes.max(1)))
    }

    pub(crate) fn full(&self, max_bytes: u64, max_records: u64) -> bool {
        self.close_early || self.bytes >= max_bytes || self.records >= max_records
    }

    /// Fails every shard in the window that has not failed already.
    pub(crate) fn fail_all(&mut self, failure: &CommitError) {
        for ((shard, _), _) in &self.batches {
            self.failures
                .entry(shard.clone())
                .or_insert_with(|| failure.clone());
        }
    }
}
