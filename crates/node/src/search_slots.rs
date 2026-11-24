//! A gate that orders searches under pressure: a bounded number of permits,
//! a fair share of them for one tenant while the gate is full, and a short
//! wait for a tenant over its share, never a refusal.
//!
//! The rules are those of ADR 0047, ported in spirit from the shell's
//! `SearchAdmission`. A search that finds no permit it may wait for runs
//! anyway as an overflow runner holding none; the REST server's worker pool
//! stays the hard bound.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How the gate is sized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// The permits; 0 is no gate.
    pub slots: usize,
    /// The share of the slots one tenant may hold while the gate is full, as
    /// a percentage.
    pub fair_share_pct: usize,
    /// The longest a search waits for a permit, in milliseconds.
    pub wait_millis: u64,
    /// The most searches waiting at once.
    pub waiter_cap: usize,
}

/// Permits held and searches waiting, and a tenant's share of each.
#[derive(Default)]
struct State {
    in_use: usize,
    waiting: usize,
    /// Tenant to (permits held, searches waiting); gone when both are 0.
    tenants: HashMap<String, (usize, usize)>,
}

/// What the gate has done, for the usage route.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The permits.
    pub slots: usize,
    /// Permits held now.
    pub in_use: usize,
    /// Searches waiting now.
    pub waiting: usize,
    /// Tenants with a permit held or a search waiting now.
    pub tenants: usize,
    /// Permits taken since the start.
    pub admitted: u64,
    /// Searches that waited.
    pub waited: u64,
    /// Searches that ran after the wait ended without a permit.
    pub past_wait: u64,
    /// Searches that ran without a permit because the gate was full.
    pub overflow_full: u64,
    /// Searches that ran without waiting because the waiters were at the cap.
    pub overflow_cap: u64,
    /// The longest wait seen, in milliseconds.
    pub longest_wait_millis: u64,
}

/// The gate.
pub struct SearchSlots {
    config: Config,
    state: Mutex<State>,
    wake: Condvar,
    admitted: AtomicU64,
    waited: AtomicU64,
    past_wait: AtomicU64,
    overflow_full: AtomicU64,
    overflow_cap: AtomicU64,
    longest: AtomicU64,
}

/// A search admitted: it gives its permit back, if it holds one, on drop.
pub struct Admission<'a> {
    gate: &'a SearchSlots,
    /// The tenant, when a permit is held.
    permit: Option<String>,
}

impl Admission<'_> {
    /// Whether this search holds a permit (it does not when it runs as an
    /// overflow runner).
    pub fn holds_permit(&self) -> bool {
        self.permit.is_some()
    }
}

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        let Some(tenant) = self.permit.take() else {
            return;
        };
        let mut st = self.gate.lock();
        st.in_use = st.in_use.saturating_sub(1);
        if let Some(entry) = st.tenants.get_mut(&tenant) {
            entry.0 = entry.0.saturating_sub(1);
            if *entry == (0, 0) {
                st.tenants.remove(&tenant);
            }
        }
        drop(st);
        self.gate.wake.notify_all();
    }
}

impl SearchSlots {
    /// A gate sized by `config`.
    pub fn new(config: &Config) -> Self {
        Self {
            config: *config,
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            admitted: AtomicU64::new(0),
            waited: AtomicU64::new(0),
            past_wait: AtomicU64::new(0),
            overflow_full: AtomicU64::new(0),
            overflow_cap: AtomicU64::new(0),
            longest: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The most permits one tenant may hold while the gate is full.
    fn share_cap(&self) -> usize {
        let share = self
            .config
            .slots
            .saturating_mul(self.config.fair_share_pct)
            .checked_div(100)
            .unwrap_or(0);
        share.max(1)
    }

    /// Whether a tenant at `level` is held to half the slots, and `st` shows
    /// another tenant active.
    fn level_rule(&self, st: &State, tenant: &str, level: u8) -> bool {
        level >= 2 && st.in_use >= self.config.slots.div_ceil(2) && Self::others(st, tenant)
    }

    /// Whether a tenant other than `tenant` holds a permit or waits; the
    /// counts of `tenant` include a search of its own that waits.
    fn others(st: &State, tenant: &str) -> bool {
        let (held, waiting) = st.tenants.get(tenant).copied().unwrap_or((0, 0));
        st.in_use > held || st.waiting > waiting
    }

    fn take(&self, st: &mut State, tenant: &str) -> Admission<'_> {
        st.in_use = st.in_use.saturating_add(1);
        self.admitted.fetch_add(1, Ordering::Relaxed);
        let entry = st.tenants.entry(tenant.to_owned()).or_default(); // ALLOC: one key a tenant with a permit.
        entry.0 = entry.0.saturating_add(1);
        Admission {
            gate: self,
            permit: Some(tenant.to_owned()), // ALLOC: one key, kept to give the permit back.
        }
    }

    const fn overflow(&self) -> Admission<'_> {
        Admission {
            gate: self,
            permit: None,
        }
    }

    /// Admits a search of `tenant`, which is at `level`: with a permit
    /// at once, after a wait, or as an overflow runner with none.
    pub fn admit(&self, tenant: &str, level: u8) -> Admission<'_> {
        let mut st = self.lock();
        let (held, _) = st.tenants.get(tenant).copied().unwrap_or((0, 0));
        let level_rule = self.level_rule(&st, tenant, level);
        if st.in_use < self.config.slots && !level_rule {
            return self.take(&mut st, tenant);
        }
        let over_share =
            level_rule || (held.saturating_add(1) > self.share_cap() && Self::others(&st, tenant));
        if !over_share {
            self.overflow_full.fetch_add(1, Ordering::Relaxed);
            return self.overflow();
        }
        if st.waiting >= self.config.waiter_cap {
            self.overflow_cap.fetch_add(1, Ordering::Relaxed);
            return self.overflow();
        }
        self.wait(st, tenant, level)
    }

    fn wait(&self, mut st: MutexGuard<'_, State>, tenant: &str, level: u8) -> Admission<'_> {
        let arrived = Instant::now();
        let deadline = arrived
            .checked_add(Duration::from_millis(self.config.wait_millis))
            .unwrap_or(arrived);
        st.waiting = st.waiting.saturating_add(1);
        let entry = st.tenants.entry(tenant.to_owned()).or_default(); // ALLOC: one key a waiting tenant.
        entry.1 = entry.1.saturating_add(1);
        self.waited.fetch_add(1, Ordering::Relaxed);
        let admitted = loop {
            if st.in_use < self.config.slots && !self.level_rule(&st, tenant, level) {
                break true;
            }
            let now = Instant::now();
            if now >= deadline {
                break false;
            }
            st = self
                .wake
                .wait_timeout(st, deadline.saturating_duration_since(now))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        };
        st.waiting = st.waiting.saturating_sub(1);
        if let Some(entry) = st.tenants.get_mut(tenant) {
            entry.1 = entry.1.saturating_sub(1);
            if *entry == (0, 0) {
                st.tenants.remove(tenant);
            }
        }
        let millis = u64::try_from(arrived.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.longest.fetch_max(millis, Ordering::Relaxed);
        if admitted {
            self.take(&mut st, tenant)
        } else {
            self.past_wait.fetch_add(1, Ordering::Relaxed);
            self.overflow()
        }
    }

    /// What the gate holds and has done.
    pub fn report(&self) -> Report {
        let st = self.lock();
        Report {
            slots: self.config.slots,
            in_use: st.in_use,
            waiting: st.waiting,
            tenants: st.tenants.len(),
            admitted: self.admitted.load(Ordering::Relaxed),
            waited: self.waited.load(Ordering::Relaxed),
            past_wait: self.past_wait.load(Ordering::Relaxed),
            overflow_full: self.overflow_full.load(Ordering::Relaxed),
            overflow_cap: self.overflow_cap.load(Ordering::Relaxed),
            longest_wait_millis: self.longest.load(Ordering::Relaxed),
        }
    }
}

/// What names the tenant and level of a search by its index name.
pub type Lane = Box<dyn Fn(&str) -> (String, u8) + Send + Sync>;

/// A gate with what names the tenant and level of a search by its index.
pub struct Gated {
    gate: std::sync::Arc<SearchSlots>,
    lane: Lane,
}

impl Gated {
    /// `gate` ordering searches by the `(tenant, level)` that `lane` gives
    /// for an index name.
    pub fn new(gate: std::sync::Arc<SearchSlots>, lane: Lane) -> Self {
        Self { gate, lane }
    }

    /// The gate, for reports.
    pub fn gate(&self) -> &std::sync::Arc<SearchSlots> {
        &self.gate
    }

    /// Admits a search of the index in `path` (the first plain name of a
    /// comma list; a wildcard or an empty name is not gated), `None` when
    /// it is not gated. The returned guard holds the permit, if any.
    pub fn admit(&self, path: &str) -> Option<Admission<'_>> {
        let name = path.split(',').next().map(str::trim)?;
        if name.is_empty() || name.contains(['*', '?']) || name.starts_with('_') {
            return None;
        }
        let (tenant, level) = (self.lane)(name);
        Some(self.gate.admit(&tenant, level))
    }
}
