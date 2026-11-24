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

/// The cost tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// 20 cost units a minute by default.
    Small,
    /// 200 by default.
    Standard,
    /// 2,000 by default.
    Large,
}

impl Tier {
    /// `small`, `standard` or `large`.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "small" => Some(Self::Small),
            "standard" => Some(Self::Standard),
            "large" => Some(Self::Large),
            _ => None,
        }
    }
}

/// The budget of each tier, in cost units a minute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierBudgets {
    /// The small tier.
    pub small: u64,
    /// The standard tier.
    pub standard: u64,
    /// The large tier.
    pub large: u64,
}

impl Default for TierBudgets {
    fn default() -> Self {
        Self {
            small: 20,
            standard: 200,
            large: 2_000,
        }
    }
}

impl TierBudgets {
    fn of(self, tier: Tier) -> u64 {
        match tier {
            Tier::Small => self.small,
            Tier::Standard => self.standard,
            Tier::Large => self.large,
        }
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
    tiers: TierBudgets,
    default_budget: u64,
    /// Each index's own budget, where it sets a tier or a budget.
    budgets: RwLock<HashMap<String, u64>>,
    /// The budget each tenant was last judged against.
    judged: Mutex<HashMap<String, u64>>,
}

impl TenantLevels {
    /// Levels over `ledger` in `mode`, every tenant held to
    /// `budget_per_minute` cost units a minute (the large tier).
    pub fn new(ledger: Arc<Ledger>, mode: Mode, budget_per_minute: u64) -> Self {
        let tiers = TierBudgets {
            large: budget_per_minute,
            ..TierBudgets::default()
        };
        Self {
            guard: Guardrails::new(MAX_WAIT_MILLIS),
            mode,
            ledger,
            groups: RwLock::new(HashMap::new()),
            docs: Mutex::new(HashMap::new()),
            tiers,
            default_budget: budget_per_minute,
            budgets: RwLock::new(HashMap::new()),
            judged: Mutex::new(HashMap::new()),
        }
    }

    /// These levels with `tiers` as the tier budgets and `default` as the
    /// tier of an index that sets none.
    #[must_use]
    pub fn with_tiers(mut self, tiers: TierBudgets, default: Tier) -> Self {
        self.tiers = tiers;
        self.default_budget = tiers.of(default);
        self
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

    /// Remembers the billing settings of `index`: its group, and the budget
    /// its budget or its tier names.
    pub fn note_tenancy(&self, index: &str, tenancy: &crate::writer::Tenancy) {
        self.note_group(index, tenancy.group.as_deref().unwrap_or(""));
        let own = tenancy
            .budget
            .or_else(|| Tier::parse(tenancy.tier.as_deref()?).map(|t| self.tiers.of(t)));
        let mut budgets = self.budgets.write().unwrap_or_else(PoisonError::into_inner);
        match own {
            Some(budget) => {
                if budgets.len() < ROSTER_LIMIT || budgets.contains_key(index) {
                    // ALLOC: one name an index with a budget, bounded by the limit.
                    budgets.insert(index.to_owned(), budget);
                }
            }
            None => {
                budgets.remove(index);
            }
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
        let mut budget: HashMap<String, u64> = HashMap::new();
        let own = self.budgets.read().unwrap_or_else(PoisonError::into_inner);
        for (index, usage) in self.ledger.lines() {
            if index == NODE || index == OTHER {
                continue;
            }
            let tenant = self.tenant_of(&index);
            let mine = own.get(&index).copied().unwrap_or(self.default_budget);
            let largest = budget.entry(tenant.clone()).or_insert(0); // ALLOC: one name a tenant, a tick.
            *largest = (*largest).max(mine);
            let entry = cost.entry(tenant).or_insert(0);
            *entry = entry.saturating_add(usage.cost_milli_units());
        }
        drop(own);
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
                budget_per_minute: budget.get(tenant).copied().unwrap_or(self.default_budget),
            })
            .collect(); // ALLOC: one measure a tenant, a tick.
        self.guard.tick(now, &measures);
        *self.judged.lock().unwrap_or_else(PoisonError::into_inner) = measures
            .iter()
            .map(|m| (m.tenant.to_owned(), m.budget_per_minute)) // ALLOC: one name a tenant, a tick.
            .collect();
    }

    /// The tenant of `index` and the level a search of it is held at: 0
    /// unless levels are enforced.
    pub fn search_lane(&self, index: &str, now: i64) -> (String, u8) {
        let tenant = self.tenant_of(index);
        let level = if self.mode == Mode::Enforce {
            self.guard.level(&tenant, now)
        } else {
            0
        };
        (tenant, level)
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

    /// The budget `tenant` was last judged against, in cost units a minute.
    pub fn budget_of(&self, tenant: &str) -> u64 {
        self.judged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(tenant)
            .copied()
            .unwrap_or(self.default_budget)
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
