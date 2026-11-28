//! What a plugin may ask the host for during a call, and the rules that
//! hold it to what it was granted (docs/specs/plugin-system AC-4, AC-14;
//! [ADR 0080](../../../docs/adr/0080-one-plugin-system-three-tiers.md), section 4).
//!
//! The plugin sees a [`Host`]: one `call(op, bytes)` whose operation names
//! are versioned data, so a new operation is an additive change. The node
//! supplies a [`Backend`] that does the work; [`CallHost`] stands between
//! them and judges every operation the same way for every tier: is the
//! capability granted, is the call's budget spent, is the answer within its
//! ceiling. This crate holds no runtime and does no I/O.

use std::sync::Mutex;

use crate::capability::{Budget, Capability};

/// What a refused or failed host operation was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    /// The host has no operation of that name.
    Unknown,
    /// The capability it needs was not granted.
    Denied,
    /// The call's budget for that kind of operation is spent.
    Budget,
    /// The answer would be over the ceiling the plugin gave.
    TooLarge,
    /// The operation's input is not what it takes.
    Invalid,
    /// This node has nothing behind the operation.
    Unavailable,
    /// The operation ran and failed.
    Failed,
}

impl HostError {
    /// The HTTP status a route answers when the plugin passes this on.
    pub fn status(self) -> u16 {
        match self {
            HostError::Unknown | HostError::Invalid => 400,
            HostError::Denied => 403,
            HostError::Budget => 429,
            HostError::TooLarge => 413,
            HostError::Unavailable => 503,
            HostError::Failed => 500,
        }
    }
}

/// What a plugin calls, from inside a call.
pub trait Host {
    /// Runs `op` with `input`, answering at most `max_out` bytes.
    ///
    /// # Errors
    ///
    /// Why the host refused or failed the operation.
    fn call(&self, op: &str, input: &[u8], max_out: usize) -> Result<Vec<u8>, HostError>;
}

/// A host with nothing to offer: every operation is refused.
pub struct NoHost;

impl Host for NoHost {
    fn call(&self, _op: &str, _input: &[u8], _max_out: usize) -> Result<Vec<u8>, HostError> {
        Err(HostError::Denied)
    }
}

/// Who an operation is for.
#[derive(Debug, Clone, Copy)]
pub struct Who<'a> {
    /// The plugin's id.
    pub plugin: &'a str,
    /// The tenant the call serves, empty when none.
    pub tenant: &'a str,
    /// What the plugin was granted; an egress operation checks its host here.
    pub grants: &'a [Capability],
}

/// The node's side: does what an operation names, for `who`.
pub trait Backend: Send + Sync {
    /// Runs `op` with `input`. The caller has already judged the grant and
    /// the budget.
    ///
    /// # Errors
    ///
    /// Why the operation failed.
    fn call(&self, who: &Who<'_>, op: &str, input: &[u8]) -> Result<Vec<u8>, HostError>;
}

/// A backend with nothing behind it.
pub struct NoBackend;

impl Backend for NoBackend {
    fn call(&self, _who: &Who<'_>, _op: &str, _input: &[u8]) -> Result<Vec<u8>, HostError> {
        Err(HostError::Unavailable)
    }
}

/// Which budget item an operation spends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spends {
    Nothing,
    State,
    Search,
    Egress,
}

/// One operation: name, the capability it needs, what it spends.
type Op = (&'static str, fn(&Capability) -> bool, Spends);

/// The operations of API 1.0.
const OPS: &[Op] = &[
    (
        "state.get",
        |c| matches!(c, Capability::StateRead),
        Spends::State,
    ),
    (
        "state.put",
        |c| matches!(c, Capability::StateWrite),
        Spends::State,
    ),
    (
        "state.delete",
        |c| matches!(c, Capability::StateWrite),
        Spends::State,
    ),
    (
        "search",
        |c| matches!(c, Capability::Search),
        Spends::Search,
    ),
    (
        "job.submit",
        |c| matches!(c, Capability::Job),
        Spends::Search,
    ),
    ("job.get", |c| matches!(c, Capability::Job), Spends::Nothing),
    (
        "job.delete",
        |c| matches!(c, Capability::Job),
        Spends::Nothing,
    ),
    (
        "job.stats",
        |c| matches!(c, Capability::Job),
        Spends::Nothing,
    ),
    (
        "egress",
        |c| matches!(c, Capability::Egress(_)),
        Spends::Egress,
    ),
    (
        "identity",
        |c| matches!(c, Capability::Identity),
        Spends::Nothing,
    ),
    ("log", |c| matches!(c, Capability::Log), Spends::Nothing),
    (
        "metrics",
        |c| matches!(c, Capability::Metrics),
        Spends::Nothing,
    ),
];

/// The names of every operation, for a tier's adapter to bind.
pub fn operations() -> impl Iterator<Item = &'static str> {
    OPS.iter().map(|(name, _, _)| *name)
}

/// One call's host: judges each operation, then hands it to the backend.
pub struct CallHost<'a> {
    who: Who<'a>,
    budget: Budget,
    used: Mutex<[u32; 3]>,
    backend: &'a dyn Backend,
}

impl<'a> CallHost<'a> {
    /// A host for one call of `who`, held to `budget`.
    pub fn new(who: Who<'a>, budget: Budget, backend: &'a dyn Backend) -> Self {
        Self {
            who,
            budget,
            used: Mutex::new([0; 3]),
            backend,
        }
    }

    fn spend(&self, item: Spends) -> Result<(), HostError> {
        let (slot, limit) = match item {
            Spends::Nothing => return Ok(()),
            Spends::State => (0, self.budget.state_ops),
            Spends::Search => (1, self.budget.searches),
            Spends::Egress => (2, self.budget.egress),
        };
        let mut used = self
            .used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = used.get_mut(slot).ok_or(HostError::Failed)?;
        if *count >= limit {
            return Err(HostError::Budget);
        }
        *count = count.saturating_add(1);
        Ok(())
    }
}

impl Host for CallHost<'_> {
    fn call(&self, op: &str, input: &[u8], max_out: usize) -> Result<Vec<u8>, HostError> {
        let (_, needs, spends) = OPS
            .iter()
            .find(|(name, _, _)| *name == op)
            .ok_or(HostError::Unknown)?;
        if !self.who.grants.iter().any(needs) {
            return Err(HostError::Denied);
        }
        self.spend(*spends)?;
        let out = self.backend.call(&self.who, op, input)?;
        if out.len() > max_out {
            return Err(HostError::TooLarge);
        }
        Ok(out)
    }
}
