//! Tenant levels on a node: each tenant's cost from the usage ledger, rolled
//! up and judged against a budget every few seconds.
//!
//! A tenant is the group an index is bucketed with
//! (`index.serverless.bucket_group`), else the index itself. The policy is
//! [`oss_blob::guard::Guardrails`]; this module feeds it from the ledger on a
//! timer and reports what it computed. In `observe` mode nothing slows a
//! tenant.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};
use std::time::Duration;

use oss_blob::guard::{Guardrails, Measure, Mode, TenantReport};
use oss_blob::usage::{Ledger, NODE, OTHER};

/// The longest a paced write waits before it is refused.
const MAX_WAIT_MILLIS: u64 = 5_000;

/// By how much a tenant at level 1 or above has its publish age stretched.
const PUBLISH_STRETCH: u64 = 4;

/// The most indices whose group is remembered.
const ROSTER_LIMIT: usize = 100_000;

/// `off`, `observe` or `enforce`.
pub fn word(mode: Mode) -> &'static str {
    match mode {
        Mode::Off => "off",
        Mode::Observe => "observe",
        Mode::Enforce => "enforce",
    }
}

/// A node's tenant levels.
pub struct TenantLevels {
    guard: Guardrails,
    mode: Mode,
    ledger: Arc<Ledger>,
    groups: RwLock<HashMap<String, String>>,
    /// Documents written so far, by index.
    docs: Mutex<HashMap<String, Arc<AtomicU64>>>,
    budget_per_minute: u64,
}

impl TenantLevels {
    /// Levels over `ledger` in `mode`, every tenant held to
    /// `budget_per_minute` cost units a minute.
    pub fn new(ledger: Arc<Ledger>, mode: Mode, budget_per_minute: u64) -> Self {
        Self {
            guard: Guardrails::new(MAX_WAIT_MILLIS),
            mode,
            ledger,
            groups: RwLock::new(HashMap::new()),
            docs: Mutex::new(HashMap::new()),
            budget_per_minute,
        }
    }

    /// The mode in force.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Remembers that `index` is bucketed with `group`; an empty group is
    /// the index itself. Not remembered past 100,000 indices.
    pub fn note_group(&self, index: &str, group: &str) {
        let mut groups = self.groups.write().unwrap_or_else(PoisonError::into_inner);
        if group.is_empty() || group == index {
            groups.remove(index);
        } else if groups.len() < ROSTER_LIMIT || groups.contains_key(index) {
            // ALLOC: two names an index with a group, bounded by the limit.
            groups.insert(index.to_owned(), group.to_owned());
        }
    }

    /// The tenant `index` belongs to.
    pub fn tenant_of(&self, index: &str) -> String {
        self.groups
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(index)
            .cloned()
            .unwrap_or_else(|| index.to_owned()) // ALLOC: one name a call.
    }

    /// Reads the ledger and steps every tenant's level to `now`.
    pub fn tick(&self, now: i64) {
        if self.mode == Mode::Off {
            return;
        }
        // ALLOC: one entry a tenant, bounded by the ledger's lines, a tick.
        let mut cost: HashMap<String, u64> = HashMap::new();
        for (index, usage) in self.ledger.lines() {
            if index == NODE || index == OTHER {
                continue;
            }
            let entry = cost.entry(self.tenant_of(&index)).or_insert(0);
            *entry = entry.saturating_add(usage.cost_milli_units());
        }
        let mut docs: HashMap<String, u64> = HashMap::new(); // ALLOC: one entry a tenant, a tick.
        for (index, count) in self
            .docs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            let entry = docs.entry(self.tenant_of(index)).or_insert(0);
            *entry = entry.saturating_add(count.load(Ordering::Relaxed));
        }
        // A tenant that wrote documents and made no request yet still counts.
        for tenant in docs.keys() {
            cost.entry(tenant.clone()).or_insert(0); // ALLOC: one name a tenant, a tick.
        }
        let measures: Vec<Measure<'_>> = cost
            .iter()
            .map(|(tenant, cost_milli)| Measure {
                tenant,
                cost_milli: *cost_milli,
                docs: docs.get(tenant).copied().unwrap_or(0),
                budget_per_minute: self.budget_per_minute,
            })
            .collect(); // ALLOC: one measure a tenant, a tick.
        self.guard.tick(now, &measures);
    }

    /// How many times longer `index`'s writes may wait to be published: 4 at
    /// level 1 or above when levels are enforced, else 1.
    pub fn publish_stretch(&self, index: &str, now: i64) -> u64 {
        if self
            .guard
            .enforced_at(self.mode, &self.tenant_of(index), 1, now)
        {
            PUBLISH_STRETCH
        } else {
            1
        }
    }

    /// Counts `docs` documents written to `index` and says how long to hold
    /// the write: milliseconds to wait, or the milliseconds a refused write
    /// would have waited. Holds only an enforced tenant at level 3 over its
    /// budget.
    ///
    /// # Errors
    ///
    /// The wait, in milliseconds, of a write that would wait past the
    /// longest wait.
    pub fn admit_write(&self, index: &str, docs: usize, now: i64) -> Result<u64, u64> {
        let count = u64::try_from(docs).unwrap_or(u64::MAX);
        if self.mode == Mode::Off {
            return Ok(0);
        }
        {
            let mut seen = self.docs.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(total) = seen.get(index) {
                total.fetch_add(count, Ordering::Relaxed);
            } else if seen.len() < ROSTER_LIMIT {
                // ALLOC: one counter and one name an index written to, bounded by the limit.
                seen.insert(index.to_owned(), Arc::new(AtomicU64::new(count)));
            }
        }
        self.guard
            .pace_write(self.mode, &self.tenant_of(index), count, now)
            .map_err(|refused| refused.retry_after_millis)
    }

    /// Takes the levels a collector set from outside (`tenant=level,...`).
    pub fn set_levels(&self, text: &str) {
        self.guard.set_levels(text);
    }

    /// Every tenant this node has seen, by name.
    pub fn report(&self, now: i64) -> Vec<(String, TenantReport)> {
        self.guard.report(now)
    }

    /// Steps the levels every `every` until the ledger is gone, which is
    /// when the node that made it has stopped.
    pub fn spawn_ticker(
        self: &Arc<Self>,
        every: Duration,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) {
        let levels = Arc::clone(self);
        let alive: Weak<Ledger> = Arc::downgrade(&self.ledger);
        // A node that cannot start the thread simply shows no levels.
        let _ = std::thread::Builder::new()
            .name("tenant-levels".to_owned())
            .spawn(move || {
                let slice = Duration::from_millis(20).min(every);
                'run: loop {
                    let mut waited = Duration::ZERO;
                    while waited < every {
                        std::thread::sleep(slice);
                        waited = waited.saturating_add(slice);
                        // The levels hold the ledger themselves: once they
                        // are its only holder, the node that made it has
                        // stopped.
                        if alive.strong_count() <= 1 {
                            break 'run;
                        }
                    }
                    levels.tick(clock());
                }
            });
    }
}
