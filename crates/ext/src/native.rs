//! The native form of a plugin and the guarded way every call to it is made
//! (docs/specs/plugin-system AC-7, AC-10, AC-11).
//!
//! A [`NativeInstance`] is code in this process, linked in or loaded from a
//! library behind the same trait. The [`Guard`] puts the bulkhead, the circuit
//! breaker and `catch_unwind` around every call, and holds no lock while the
//! plugin runs.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;

use crate::breaker::{Breaker, BreakerConfig, BreakerState, Decision, Outcome};
use crate::error::{CallError, FailureKind};

/// The most calls to one plugin at once.
pub const MAX_CONCURRENT: usize = 8;
/// The most calls for one tenant at once.
pub const MAX_PER_TENANT: usize = 2;

/// A plugin that runs in this process.
pub trait NativeInstance: Send + Sync {
    /// The manifest this plugin declares, as the parser reads it.
    fn manifest(&self) -> Vec<u8>;

    /// Serves one call of `point` with `input`, answering at most `max_out`
    /// bytes.
    ///
    /// # Errors
    ///
    /// The failure, typed as a module's status is.
    fn call(&self, point: &str, input: &[u8], max_out: usize) -> Result<Vec<u8>, CallError>;
}

/// Why a guarded call did not return the plugin's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardError {
    /// Eight calls to the plugin are already running.
    PluginBusy,
    /// Two calls for the tenant are already running.
    TenantBusy,
    /// The breaker is open.
    BreakerOpen,
    /// The plugin panicked.
    Panicked,
    /// The plugin answered with a failure, or an output over its ceiling.
    Failed(CallError),
}

impl GuardError {
    /// The HTTP status the failure answers.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::PluginBusy | Self::TenantBusy => 429,
            Self::BreakerOpen => 503,
            Self::Panicked => 500,
            Self::Failed(e) => e.http_status(),
        }
    }

    /// The error type the failure answers with.
    pub fn code(&self) -> &'static str {
        match self {
            Self::PluginBusy | Self::TenantBusy => "plugin_busy",
            Self::BreakerOpen => "plugin_unavailable",
            Self::Panicked => "plugin_failed",
            Self::Failed(e) => match e.kind() {
                FailureKind::Malformed | FailureKind::NotInitialized | FailureKind::Unsupported => {
                    "plugin_malformed"
                }
                FailureKind::Busy => "plugin_busy",
                FailureKind::ModuleFailed | FailureKind::Trap => "plugin_failed",
                _ => "plugin_rejected",
            },
        }
    }
}

struct Gate {
    breaker: Breaker,
    running: usize,
    tenants: HashMap<String, usize>,
}

/// The bulkhead, breaker and panic fence for one plugin.
pub struct Guard {
    gate: Mutex<Gate>,
}

/// Frees a bulkhead place when the call ends, however it ends.
struct Place<'a> {
    guard: &'a Guard,
    tenant: &'a str,
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        let mut g = self.guard.lock();
        g.running = g.running.saturating_sub(1);
        if self.tenant.is_empty() {
            return;
        }
        if let Some(n) = g.tenants.get_mut(self.tenant) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                g.tenants.remove(self.tenant);
            }
        }
    }
}

fn outcome_of(error: &GuardError) -> Outcome {
    match error {
        GuardError::Panicked => Outcome::Failure,
        GuardError::Failed(e) => match e.kind() {
            FailureKind::ModuleFailed
            | FailureKind::Trap
            | FailureKind::Malformed
            | FailureKind::NotInitialized
            | FailureKind::OutputTooLarge
            | FailureKind::Cancelled => Outcome::Failure,
            _ => Outcome::Neutral,
        },
        _ => Outcome::Neutral,
    }
}

impl Guard {
    /// A guard whose breaker has `config` and jitters from `seed`.
    pub fn new(config: BreakerConfig, seed: u64) -> Self {
        Self {
            gate: Mutex::new(Gate {
                breaker: Breaker::new(config, seed),
                running: 0,
                // ALLOC: at most MAX_CONCURRENT entries, each removed at zero.
                tenants: HashMap::new(),
            }),
        }
    }

    /// The lock, recovered if a panic poisoned it: the state it guards is
    /// plain counters, valid after any panic.
    fn lock(&self) -> std::sync::MutexGuard<'_, Gate> {
        self.gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Where the breaker is.
    pub fn state(&self) -> BreakerState {
        self.lock().breaker.state()
    }

    /// How many tenants have a call running now.
    pub fn tenants_in_flight(&self) -> usize {
        self.lock().tenants.len()
    }

    /// Calls `plugin` for `tenant` at `now_ms`. No lock is held while the
    /// plugin runs; a panic is caught and counted as a failure.
    ///
    /// # Errors
    ///
    /// Why the call was refused or failed.
    pub fn call(
        &self,
        plugin: &dyn NativeInstance,
        tenant: &str,
        point: &str,
        input: &[u8],
        max_out: usize,
        now_ms: impl Fn() -> u64,
    ) -> Result<Vec<u8>, GuardError> {
        let decision = {
            let mut g = self.lock();
            if g.running >= MAX_CONCURRENT {
                return Err(GuardError::PluginBusy);
            }
            // A call with no tenant is held to the plugin's limit only.
            if !tenant.is_empty() && g.tenants.get(tenant).copied().unwrap_or(0) >= MAX_PER_TENANT {
                return Err(GuardError::TenantBusy);
            }
            let decision = g.breaker.allow(now_ms());
            if decision == Decision::Reject {
                return Err(GuardError::BreakerOpen);
            }
            g.running = g.running.saturating_add(1);
            if !tenant.is_empty() {
                // ALLOC: one entry for a tenant with a call running.
                let n = g.tenants.entry(tenant.to_owned()).or_insert(0);
                *n = n.saturating_add(1);
            }
            decision
        };
        let _place = Place {
            guard: self,
            tenant,
        };
        let result = match catch_unwind(AssertUnwindSafe(|| plugin.call(point, input, max_out))) {
            Err(_) => Err(GuardError::Panicked),
            Ok(Err(e)) => Err(GuardError::Failed(e)),
            Ok(Ok(out)) if out.len() > max_out => Err(GuardError::Failed(
                CallError::malformed_because("output over its ceiling"),
            )),
            Ok(Ok(out)) => Ok(out),
        };
        let outcome = match &result {
            Ok(_) => Outcome::Success,
            Err(e) => outcome_of(e),
        };
        self.lock().breaker.record(decision, outcome, now_ms());
        result
    }
}
