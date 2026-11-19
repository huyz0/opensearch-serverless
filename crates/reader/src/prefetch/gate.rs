//! The node-wide bound on what open prefetches hold and run.
//!
//! Every open takes its bytes and threads from one [`Gate`] before it
//! allocates or spawns; a refusal only leaves a range to the engine, so
//! however many shards open at once, the prefetch holds at most
//! [`Gate::max_bytes`] and runs at most [`Gate::max_threads`] fetches.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// A counted pool of bytes and fetch threads, taken without waiting.
#[derive(Debug)]
pub struct Gate {
    max_bytes: u64,
    max_threads: u64,
    bytes: AtomicU64,
    threads: AtomicU64,
}

/// What a [`Gate`] granted; given back on drop.
#[derive(Debug)]
pub struct Grant {
    gate: Arc<Gate>,
    bytes: u64,
    threads: u64,
}

impl Gate {
    /// A gate of `max_bytes` and `max_threads`.
    pub fn new(max_bytes: u64, max_threads: u64) -> Arc<Self> {
        Arc::new(Self {
            max_bytes,
            max_threads,
            bytes: AtomicU64::new(0),
            threads: AtomicU64::new(0),
        })
    }

    /// The gate every open shares unless it is given another: 32 MiB and 32
    /// threads for the node.
    pub fn node() -> Arc<Self> {
        static NODE: OnceLock<Arc<Gate>> = OnceLock::new();
        Arc::clone(NODE.get_or_init(|| Self::new(32 << 20, 32)))
    }

    /// The most bytes held at once.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// The most fetches run at once.
    pub fn max_threads(&self) -> u64 {
        self.max_threads
    }

    /// Bytes held now.
    pub fn held_bytes(&self) -> u64 {
        self.bytes.load(Ordering::Acquire)
    }

    /// Fetches running now.
    pub fn held_threads(&self) -> u64 {
        self.threads.load(Ordering::Acquire)
    }

    /// One thread and `bytes`, or `None` when either would pass its limit.
    pub fn try_take(self: &Arc<Self>, bytes: u64) -> Option<Grant> {
        take(&self.threads, 1, self.max_threads)?;
        if take(&self.bytes, bytes, self.max_bytes).is_none() {
            self.threads.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Grant {
            gate: Arc::clone(self),
            bytes,
            threads: 1,
        })
    }
}

fn take(counter: &AtomicU64, n: u64, max: u64) -> Option<()> {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |held| {
            held.checked_add(n).filter(|total| *total <= max)
        })
        .ok()
        .map(|_| ())
}

impl Grant {
    /// Gives the thread back, keeping the bytes until the grant drops: the
    /// fetch is over, its bytes are still held.
    pub fn release_thread(&mut self) {
        self.gate.threads.fetch_sub(self.threads, Ordering::AcqRel);
        self.threads = 0;
    }
}

impl Drop for Grant {
    fn drop(&mut self) {
        self.release_thread();
        self.gate.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
