//! The hook that lets a node queue searches off the workers (ADR 0056).
//!
//! The server stays blind to tenants: it asks the hook, after every other
//! check and before a body buffer is taken, whether a request may run now,
//! and parks the connection when the hook says to wait.

use std::sync::Arc;

use crate::method::Method;

/// A claim on a share of the workers, held while one request runs and
/// dropped when its response is done. What it counts is the hook's own.
pub type Permit = Box<dyn Send>;

/// What the hook decides for one request.
pub enum Decision {
    /// Serve it now, holding the permit (none for a request the hook does
    /// not count).
    Run(Option<Permit>),
    /// Park it until the tenant may run: its key is the hook's own.
    Defer(Arc<str>),
}

/// Decides which requests run now and which wait in the queue.
pub trait SearchAdmission: Send + Sync {
    /// Decides for a request, with its method and raw path. It runs on a
    /// worker, takes no long lock and does no I/O.
    fn decide(&self, method: Option<Method>, path: &str) -> Decision;

    /// A permit for a parked request of `tenant` when the tenant may run
    /// now, by the same rule as [`SearchAdmission::decide`]; none when it
    /// may not. Called with the queue's lock held, so it must not call back
    /// into the server.
    fn try_reserve(&self, tenant: &str) -> Option<Permit>;

    /// What happened to the queue, for the hook's own report. It must not
    /// block or call back into the server.
    fn note(&self, event: QueueEvent) {
        let _ = event;
    }
}

/// A change in the search queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueEvent {
    /// A search was parked.
    Deferred,
    /// A parked search resumed after waiting this many milliseconds.
    Resumed(u64),
    /// A search was answered 429 because the queue was full.
    Refused,
    /// A parked search was answered 429 at the wait.
    Expired,
    /// A parked search's peer closed.
    Dropped,
}
