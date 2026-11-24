//! Per-tenant cost levels: what each tenant's indices have cost this node,
//! against a budget, and the level that puts it at.
//!
//! Ported from the shell's `Guardrails`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/usage/Guardrails.java`),
//! without its clock or its settings: the caller names the time and the
//! budgets, so the policy is plain data in, plain data out.
//!
//! A tenant is an index's bucket group, else the index. Every tick the
//! caller gives each tenant's cumulative cost (the usage ledger's, in
//! thousandths of a PUT) and documents written. Over its budget for a minute
//! a tenant goes up a level, up another after two more minutes still over, to
//! three at most; under it for five minutes, down one. A level can also be set
//! from outside by a collector that sees every node ([`parse_levels`]); the
//! higher of the two holds, and a refusal shows as level four for a minute.
//!
//! Levels act only where the caller enforces them ([`Mode::Enforce`]); in
//! [`Mode::Observe`] they are computed and reported. At level three a write
//! is paced to what fits the budget ([`pace`]).

mod pace;

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError, RwLock};

pub use pace::Refused;

/// How long over budget before the first level.
const LEVEL_UP_AFTER_MILLIS: i64 = 60_000;
/// How long still over before each next level.
const NEXT_LEVEL_AFTER_MILLIS: i64 = 120_000;
/// How long under budget before a level comes off.
const LEVEL_DOWN_AFTER_MILLIS: i64 = 300_000;
/// How long a refusal shows as level four.
const REFUSED_SHOWS_MILLIS: i64 = 60_000;
/// The highest level a tenant's own cost reaches.
const TOP_LEVEL: u8 = 3;

/// What levels do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Nothing is computed.
    Off,
    /// Levels are computed and reported; nothing slows a tenant.
    Observe,
    /// Levels act.
    Enforce,
}

impl Mode {
    /// `off`, `observe` or `enforce`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "off" => Some(Self::Off),
            "observe" => Some(Self::Observe),
            "enforce" => Some(Self::Enforce),
            _ => None,
        }
    }
}

/// One tenant's cost since the node started, as of a tick.
#[derive(Clone, Copy, Debug)]
pub struct Measure<'a> {
    /// The tenant.
    pub tenant: &'a str,
    /// Its cumulative cost, in thousandths of a PUT.
    pub cost_milli: u64,
    /// Its cumulative documents written.
    pub docs: u64,
    /// Its budget, in cost units a minute.
    pub budget_per_minute: u64,
}

/// What one tenant has been doing, for reports.
#[derive(Clone, Debug, PartialEq)]
pub struct TenantReport {
    /// The level in force: the higher of local and set, or four after a refusal.
    pub level: u8,
    /// This node's own level.
    pub local_level: u8,
    /// The level set from outside.
    pub set_level: u8,
    /// Cost units a minute at the last tick.
    pub cost_per_minute: f64,
    /// Documents a minute at the last tick.
    pub docs_per_minute: f64,
    /// Writes held to fit the budget.
    pub paced: u64,
    /// Writes and searches refused.
    pub refused: u64,
}

#[derive(Debug)]
struct Tenant {
    last_cost_milli: Option<u64>,
    last_docs: u64,
    cost_per_minute: f64,
    docs_per_minute: f64,
    level: u8,
    over_since: Option<i64>,
    under_since: Option<i64>,
    changed_at: Option<i64>,
    refused_at: Option<i64>,
    refused: u64,
    paced: u64,
    /// Documents a second that fit the budget; infinite while under it.
    allowed_docs_per_second: f64,
    tokens: f64,
    tokens_at: Option<i64>,
}

impl Tenant {
    fn new() -> Self {
        Self {
            last_cost_milli: None,
            last_docs: 0,
            cost_per_minute: 0.0,
            docs_per_minute: 0.0,
            level: 0,
            over_since: None,
            under_since: None,
            changed_at: None,
            refused_at: None,
            refused: 0,
            paced: 0,
            allowed_docs_per_second: f64::INFINITY,
            tokens: 0.0,
            tokens_at: None,
        }
    }
}

/// One node's tenant levels.
#[derive(Debug)]
pub struct Guardrails {
    max_wait_millis: u64,
    tenants: Mutex<HashMap<String, Tenant>>,
    set: RwLock<HashMap<String, u8>>,
    last_tick: Mutex<Option<i64>>,
}

impl Guardrails {
    /// Levels where a write held longer than `max_wait_millis` is refused.
    pub fn new(max_wait_millis: u64) -> Self {
        Self {
            max_wait_millis,
            tenants: Mutex::new(HashMap::new()), // ALLOC: one empty map, at start.
            set: RwLock::new(HashMap::new()),    // ALLOC: one empty map, at start.
            last_tick: Mutex::new(None),
        }
    }

    /// Recomputes every level from the cost since the last tick. A tenant
    /// not in `measures` is forgotten: it did nothing here.
    pub fn tick(&self, now: i64, measures: &[Measure<'_>]) {
        let mut last = self
            .last_tick
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let elapsed = last.map(|at| now.saturating_sub(at).max(1));
        *last = Some(now);
        drop(last);
        let mut tenants = self.tenants.lock().unwrap_or_else(PoisonError::into_inner);
        // ALLOC: one name a tenant seen this tick, at most the ledger's lines.
        let mut kept: HashMap<String, Tenant> = HashMap::with_capacity(measures.len());
        for m in measures {
            let mut t = tenants.remove(m.tenant).unwrap_or_else(Tenant::new);
            if let (Some(elapsed), Some(before)) = (elapsed, t.last_cost_milli) {
                let minutes = f64::from(i32::try_from(elapsed).unwrap_or(i32::MAX)) / 60_000.0;
                t.cost_per_minute = m.cost_milli.saturating_sub(before) as f64 / 1_000.0 / minutes;
                t.docs_per_minute = m.docs.saturating_sub(t.last_docs) as f64 / minutes;
                let budget = m.budget_per_minute as f64;
                let over = t.cost_per_minute > budget;
                step(&mut t, over, now);
                // What fits the budget at the documents-to-cost ratio it
                // writes at now.
                t.allowed_docs_per_second =
                    if t.cost_per_minute <= budget || t.docs_per_minute <= 0.0 {
                        f64::INFINITY
                    } else {
                        t.docs_per_minute / 60.0 * budget / t.cost_per_minute
                    };
            }
            t.last_cost_milli = Some(m.cost_milli);
            t.last_docs = m.docs;
            kept.insert(m.tenant.to_owned(), t); // ALLOC: one name a tenant.
        }
        *tenants = kept;
    }

    /// The level in force for `tenant` at `now`: the higher of this node's
    /// and the one set from outside, and four for a minute after a refusal.
    pub fn level(&self, tenant: &str, now: i64) -> u8 {
        let tenants = self.tenants.lock().unwrap_or_else(PoisonError::into_inner);
        let state = tenants.get(tenant);
        let mut level = state.map_or(0, |t| t.level);
        if state
            .and_then(|t| t.refused_at)
            .is_some_and(|at| now.saturating_sub(at) < REFUSED_SHOWS_MILLIS)
        {
            level = 4;
        }
        drop(tenants);
        level.max(self.set_level(tenant))
    }

    /// Whether `tenant` is at `min_level` or above, and `mode` lets it act.
    pub fn enforced_at(&self, mode: Mode, tenant: &str, min_level: u8, now: i64) -> bool {
        mode == Mode::Enforce && self.level(tenant, now) >= min_level
    }

    /// Takes the levels set from outside (`tenant=level,...`).
    pub fn set_levels(&self, text: &str) {
        *self.set.write().unwrap_or_else(PoisonError::into_inner) = parse_levels(text);
    }

    fn set_level(&self, tenant: &str) -> u8 {
        self.set
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(tenant)
            .copied()
            .unwrap_or(0)
    }

    /// Shows a refusal of a tenant's search as level four for a minute.
    pub fn note_refused(&self, tenant: &str, now: i64) {
        let mut tenants = self.tenants.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(t) = tenants.get_mut(tenant) {
            t.refused = t.refused.saturating_add(1);
            t.refused_at = Some(now);
        }
    }

    /// What this node knows of each tenant it has seen, by name.
    pub fn report(&self, now: i64) -> Vec<(String, TenantReport)> {
        let names: Vec<String> = self
            .tenants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect(); // ALLOC: one name a tenant, once a read.
        let mut out: Vec<(String, TenantReport)> = names
            .into_iter()
            .filter_map(|name| {
                let level = self.level(&name, now);
                let set_level = self.set_level(&name);
                let tenants = self.tenants.lock().unwrap_or_else(PoisonError::into_inner);
                let t = tenants.get(&name)?;
                let report = TenantReport {
                    level,
                    local_level: t.level,
                    set_level,
                    cost_per_minute: t.cost_per_minute,
                    docs_per_minute: t.docs_per_minute,
                    paced: t.paced,
                    refused: t.refused,
                };
                drop(tenants);
                Some((name, report))
            })
            .collect(); // ALLOC: one report a tenant, once a read.
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

fn step(t: &mut Tenant, over: bool, now: i64) {
    if over {
        t.under_since = None;
        let over_since = *t.over_since.get_or_insert(now);
        let (wait, since) = if t.level == 0 {
            (LEVEL_UP_AFTER_MILLIS, over_since)
        } else {
            (
                NEXT_LEVEL_AFTER_MILLIS,
                over_since.max(t.changed_at.unwrap_or(i64::MIN)),
            )
        };
        if t.level < TOP_LEVEL && now.saturating_sub(since) >= wait {
            t.level = t.level.saturating_add(1);
            t.changed_at = Some(now);
        }
    } else {
        t.over_since = None;
        let under_since = *t.under_since.get_or_insert(now);
        let since = under_since.max(t.changed_at.unwrap_or(i64::MIN));
        if t.level > 0 && now.saturating_sub(since) >= LEVEL_DOWN_AFTER_MILLIS {
            t.level = t.level.saturating_sub(1);
            t.changed_at = Some(now);
        }
    }
}

/// Parses `tenant=level,...`, leaving out pairs that do not parse or name a
/// level above four.
pub fn parse_levels(text: &str) -> HashMap<String, u8> {
    let mut levels = HashMap::new(); // ALLOC: one entry a pair, once a setting change.
    for pair in text.split(',') {
        let Some((tenant, level)) = pair.rsplit_once('=') else {
            continue;
        };
        let tenant = tenant.trim();
        if let (false, Ok(level @ 0..=4)) = (tenant.is_empty(), level.trim().parse::<u8>()) {
            levels.insert(tenant.to_owned(), level); // ALLOC: one name a pair, once a setting change.
        }
    }
    levels
}
