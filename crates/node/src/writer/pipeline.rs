//! The groups of one shard whose records are staged in the node log and
//! not yet acknowledged: tickets that finish in the order they were taken,
//! a sticky failure, and a count of callers that need the shard quiet.
//!
//! Its mutex is a leaf: nothing takes the slot lock, or any other, while
//! holding it, so a wait here never holds up the slot.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

#[derive(Debug, Default)]
struct State {
    /// The next ticket to hand out.
    next: u64,
    /// The ticket that may finish now.
    serving: u64,
    in_flight: usize,
    failed: bool,
    quiescing: usize,
}

/// A shard's pipeline of staged groups.
#[derive(Debug, Default)]
pub(super) struct Pipeline {
    state: Mutex<State>,
    changed: Condvar,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Pipeline {
    /// Whether a write that finds the pipeline so must wait before it
    /// applies: it is at `depth`, or a caller wants the shard quiet.
    pub(super) fn must_wait(&self, depth: usize) -> bool {
        let s = lock(&self.state);
        !s.failed && (s.in_flight >= depth || s.quiescing > 0)
    }

    /// Blocks until a write may apply or the pipeline failed; the caller
    /// checks everything again under the slot lock.
    pub(super) fn wait_ready(&self, depth: usize) {
        let mut s = lock(&self.state);
        while !s.failed && (s.in_flight >= depth || s.quiescing > 0) {
            s = self.changed.wait(s).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Blocks until no group is in flight.
    pub(super) fn wait_idle(&self) {
        let mut s = lock(&self.state);
        while s.in_flight > 0 {
            s = self.changed.wait(s).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Whether a staged group failed, so that nothing more is acknowledged
    /// until the slot is released.
    pub(super) fn failed(&self) -> bool {
        lock(&self.state).failed
    }

    /// Marks the pipeline failed and wakes every waiter.
    pub(super) fn fail(&self) {
        lock(&self.state).failed = true;
        self.changed.notify_all();
    }

    /// Whether any group is in flight.
    pub(super) fn busy(&self) -> bool {
        lock(&self.state).in_flight > 0
    }

    /// Takes the next ticket, under the slot lock that staged the group.
    pub(super) fn enter(&self) -> u64 {
        let mut s = lock(&self.state);
        let ticket = s.next;
        s.next = s.next.saturating_add(1);
        s.in_flight = s.in_flight.saturating_add(1);
        ticket
    }

    /// Blocks until every earlier ticket has finished.
    pub(super) fn wait_turn(&self, ticket: u64) {
        let mut s = lock(&self.state);
        while s.serving != ticket {
            s = self.changed.wait(s).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Finishes `ticket`, which is the one serving. True when this group is
    /// the last out of a failed pipeline, and so releases the slot.
    pub(super) fn leave(&self, ticket: u64) -> bool {
        let mut s = lock(&self.state);
        debug_assert_eq!(s.serving, ticket, "tickets finish in order");
        s.serving = s.serving.saturating_add(1);
        s.in_flight = s.in_flight.saturating_sub(1);
        let last_out = s.failed && s.in_flight == 0;
        drop(s);
        self.changed.notify_all();
        last_out
    }

    /// Asks the shard to be quiet: new writes wait. Cleared when the
    /// returned guard drops.
    pub(super) fn quiesce(self: &Arc<Self>) -> Quiet {
        let mut s = lock(&self.state);
        s.quiescing = s.quiescing.saturating_add(1);
        drop(s);
        Quiet {
            pipeline: Arc::clone(self),
        }
    }
}

/// A shard held quiet; its writes resume when this drops.
pub(super) struct Quiet {
    pipeline: Arc<Pipeline>,
}

impl Drop for Quiet {
    fn drop(&mut self) {
        let mut s = lock(&self.pipeline.state);
        s.quiescing = s.quiescing.saturating_sub(1);
        drop(s);
        self.pipeline.changed.notify_all();
    }
}
