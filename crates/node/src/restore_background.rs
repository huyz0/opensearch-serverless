//! The bookkeeping of restores that run on their own thread: the switch,
//! how many may run, the bytes each holds, and the drain at shutdown.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use oss_mem::{Exhausted, MemoryBudget, Pool, Reservation};

/// The most background restores one node may be asked to run at once.
pub const MAX_RUNNING: usize = 16;

/// What a background restore is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// `serverless.snapshots.restore.background.enabled`.
    pub enabled: bool,
    /// The most restores copying at once, 1 to [`MAX_RUNNING`].
    pub max: usize,
    /// How long a drain waits for a running copy to stop.
    pub grace: Duration,
}

impl Settings {
    /// Off: every restore answers when its copy is done.
    #[must_use]
    pub const fn off() -> Self {
        Self {
            enabled: false,
            max: 2,
            grace: Duration::from_millis(30_000),
        }
    }
}

/// Why a restore may not start in the background.
#[derive(Debug)]
pub enum Refusal {
    /// The node is shutting down.
    Draining,
    /// `max` restores are running already.
    Busy,
    /// The pool cannot hold the restore's buffers.
    Pool(Exhausted),
}

/// The running restores of one node.
pub struct BackgroundRestores {
    settings: Settings,
    budget: MemoryBudget,
    running: Mutex<usize>,
    idle: Condvar,
    failed: AtomicU64,
    draining: AtomicBool,
}

/// One running restore's place and bytes, given back on drop, panic
/// included.
pub struct Slot {
    owner: Arc<BackgroundRestores>,
    _bytes: Reservation,
}

impl BackgroundRestores {
    /// Restores as `settings` allow, holding bytes from `budget`.
    #[must_use]
    pub fn new(settings: Settings, budget: MemoryBudget) -> Arc<Self> {
        Arc::new(Self {
            settings,
            budget,
            running: Mutex::new(0),
            idle: Condvar::new(),
            failed: AtomicU64::new(0),
            draining: AtomicBool::new(false),
        })
    }

    /// Whether a restore asked not to wait runs in the background.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.settings.enabled
    }

    /// Takes a place and `bytes` for a restore about to start.
    ///
    /// # Errors
    ///
    /// A [`Refusal`] while draining, at `max`, or when the pool is short.
    pub fn acquire(self: &Arc<Self>, bytes: usize) -> Result<Slot, Refusal> {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        if self.draining.load(Ordering::Acquire) {
            return Err(Refusal::Draining);
        }
        if *running >= self.settings.max {
            return Err(Refusal::Busy);
        }
        let reserved = self
            .budget
            .try_reserve(Pool::RequestBodies, bytes)
            .map_err(Refusal::Pool)?;
        *running = running.saturating_add(1);
        Ok(Slot {
            owner: Arc::clone(self),
            _bytes: reserved,
        })
    }

    /// Restores copying now.
    #[must_use]
    pub fn running(&self) -> usize {
        *self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Restores that ended in a failure or a cancel.
    #[must_use]
    pub fn failed(&self) -> u64 {
        self.failed.load(Ordering::Acquire)
    }

    /// Counts one failed restore.
    pub fn record_failure(&self) {
        self.failed.fetch_add(1, Ordering::AcqRel);
    }

    /// Whether a drain has started: a copy stops at its next file.
    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }

    /// Starts the drain and waits up to the grace for the copies to end;
    /// whether they all did.
    pub fn drain(&self) -> bool {
        let deadline = Instant::now().checked_add(self.settings.grace);
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        // Set under the lock: an acquire either ran before it or sees it.
        self.draining.store(true, Ordering::Release);
        while *running > 0 {
            let left = deadline.map_or(Duration::MAX, |d| {
                d.saturating_duration_since(Instant::now())
            });
            if left.is_zero() {
                return false;
            }
            running = self
                .idle
                .wait_timeout(running, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut running = self
            .owner
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *running = running.saturating_sub(1);
        self.owner.idle.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_mem::BudgetConfig;

    fn budget(bytes: usize) -> MemoryBudget {
        MemoryBudget::new(
            &BudgetConfig::new(bytes.saturating_mul(2))
                .with_pool(Pool::RequestBodies, bytes)
                .expect("pool"),
        )
    }

    fn on(max: usize, grace_ms: u64, bytes: usize) -> Arc<BackgroundRestores> {
        BackgroundRestores::new(
            Settings {
                enabled: true,
                max,
                grace: Duration::from_millis(grace_ms),
            },
            budget(bytes),
        )
    }

    #[test]
    fn it_is_off_until_set() {
        let s = Settings::off();
        assert!(!s.enabled);
        assert_eq!((s.max, s.grace), (2, Duration::from_millis(30_000)));
        let r = BackgroundRestores::new(s, budget(10));
        assert!(!r.enabled());
        assert_eq!((r.running(), r.failed(), r.cancelled()), (0, 0, false));
    }

    #[test]
    fn a_slot_holds_its_bytes_and_gives_them_back() {
        let r = on(2, 10, 100);
        let slot = r.acquire(60).expect("fits");
        assert_eq!(r.running(), 1);
        assert_eq!(r.budget.in_use(Pool::RequestBodies), 60);
        drop(slot);
        assert_eq!(r.running(), 0);
        assert_eq!(r.budget.in_use(Pool::RequestBodies), 0);
    }

    #[test]
    fn a_restore_past_max_is_busy_and_takes_nothing() {
        let r = on(1, 10, 100);
        let _a = r.acquire(10).expect("first");
        assert!(matches!(r.acquire(10), Err(Refusal::Busy)));
        assert_eq!(r.running(), 1);
        assert_eq!(r.budget.in_use(Pool::RequestBodies), 10);
    }

    #[test]
    fn a_pool_that_cannot_hold_it_refuses_and_takes_no_place() {
        let r = on(2, 10, 100);
        assert!(matches!(r.acquire(101), Err(Refusal::Pool(_))));
        assert_eq!(r.running(), 0);
        assert!(r.acquire(100).is_ok());
    }

    #[test]
    fn a_slot_dropped_by_a_panic_is_given_back() {
        let r = on(1, 10, 100);
        let held = Arc::clone(&r);
        let ended = std::thread::spawn(move || {
            let _slot = held.acquire(50).expect("fits");
            panic!("a restore thread failed");
        })
        .join();
        assert!(ended.is_err());
        assert_eq!(r.running(), 0);
        assert_eq!(r.budget.in_use(Pool::RequestBodies), 0);
    }

    #[test]
    fn failures_are_counted() {
        let r = on(1, 10, 10);
        r.record_failure();
        r.record_failure();
        assert_eq!(r.failed(), 2);
    }

    #[test]
    fn a_drain_with_nothing_running_ends_at_once_and_refuses_new_restores() {
        let r = on(2, 10, 100);
        assert!(!r.cancelled());
        assert!(r.drain());
        assert!(r.cancelled());
        assert!(matches!(r.acquire(1), Err(Refusal::Draining)));
        assert_eq!(r.budget.in_use(Pool::RequestBodies), 0);
    }

    #[test]
    fn a_drain_waits_for_a_copy_that_ends_within_the_grace() {
        let r = on(1, 5_000, 100);
        let slot = r.acquire(10).expect("fits");
        let ender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(slot);
        });
        assert!(r.drain());
        assert_eq!(r.running(), 0);
        ender.join().expect("ended");
    }

    #[test]
    fn a_drain_gives_up_on_a_copy_that_outlives_the_grace() {
        let r = on(1, 30, 100);
        let slot = r.acquire(10).expect("fits");
        let started = Instant::now();
        assert!(!r.drain());
        assert!(started.elapsed() >= Duration::from_millis(30));
        assert_eq!(r.running(), 1);
        drop(slot);
        assert_eq!(r.running(), 0);
    }
}
